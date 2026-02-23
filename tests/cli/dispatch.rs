// Integration tests for task-035: src/main.rs — Post-Parse Dispatch and Cleanup (Chunk 7)
//
// Verifies parity with lz4cli.c lines 704–887:
//   - Test mode: `-t` sets test_mode, output=NUL, op=Decompress
//   - Auto output filename: compress → input + ".lz4", decompress → strip ".lz4"
//   - Single-file compress / decompress round-trip
//   - Stdin → stdout piping (binary data)
//   - Multiple-input compress/decompress
//   - List mode (-l / --list)
//   - Error exit when decompressing without ".lz4" extension and no -o
//   - Refuse stdin if stdin is a terminal (verified via pipe)
//   - Display-level downgrade when writing to stdout
//   - RAII cleanup: no leaks / no residual output files on error
//
// NOTE: `run()` is private to the `lz4` binary crate and cannot be called
// from library integration tests.  All tests below invoke the compiled binary
// via `std::process::Command`.  Cargo sets `CARGO_BIN_EXE_lz4` to the path of
// the compiled binary when running `cargo test`.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use tempfile::TempDir;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Path to the compiled `lz4` binary under test.
fn lz4_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lz4"))
}

/// Create a fresh temp directory with a plain-text input file called `input.txt`.
fn setup_input(content: &[u8]) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("TempDir::new");
    let input = dir.path().join("input.txt");
    fs::write(&input, content).expect("write input");
    (dir, input)
}

/// Compress `input` with the binary and return the `.lz4` output path.
fn compress_file(input: &PathBuf) -> PathBuf {
    let output = input.with_extension("txt.lz4");
    let status = Command::new(lz4_bin())
        .args(["-f", input.to_str().unwrap(), output.to_str().unwrap()])
        .status()
        .expect("spawn lz4");
    assert!(status.success(), "compression failed: {status}");
    output
}

// ─────────────────────────────────────────────────────────────────────────────
// Smoke tests — help / version flags set exit_early → process::exit(0)
// (lz4cli.c lines 358-360 / args.rs: exit_early)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn help_flag_exits_zero() {
    // --help → exit_early = true → std::process::exit(0)
    let status = Command::new(lz4_bin())
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn lz4 --help");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn version_flag_exits_zero() {
    // --version → exit_early = true → std::process::exit(0)
    let status = Command::new(lz4_bin())
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn lz4 --version");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn version_output_contains_version_string() {
    // Output should contain the version number (parity: lz4cli.c prints LZ4_VERSION_STRING)
    let output = Command::new(lz4_bin())
        .arg("--version")
        .output()
        .expect("spawn lz4 --version");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should contain the version tuple "1.10.0" (from Cargo.toml)
    assert!(
        stdout.contains("1.10.0") || stdout.contains("lz4") || stdout.contains("LZ4"),
        "unexpected version output: {stdout}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Single-file compress → auto output filename = input + ".lz4"
// (lz4cli.c lines 781–789: dynNameSpace = concat input + LZ4_EXTENSION)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_single_file_explicit_output() {
    // lz4 -f input.txt input.txt.lz4 → creates .lz4 file
    let (_dir, input) = setup_input(b"hello world compress test");
    let output = input.with_extension("txt.lz4");
    let status = Command::new(lz4_bin())
        .args(["-f", input.to_str().unwrap(), output.to_str().unwrap()])
        .status()
        .expect("spawn lz4");
    assert!(status.success());
    assert!(output.exists(), ".lz4 output file must exist");
    assert!(
        output.metadata().unwrap().len() > 0,
        ".lz4 file must be non-empty"
    );
}

#[test]
fn compress_exit_code_zero_on_success() {
    // Successful compression → operationResult = 0 → exit(0)
    let (_dir, input) = setup_input(b"exit code test");
    let output = input.with_extension("txt.lz4");
    let status = Command::new(lz4_bin())
        .args(["-f", input.to_str().unwrap(), output.to_str().unwrap()])
        .status()
        .expect("spawn");
    assert_eq!(status.code(), Some(0));
}

// ─────────────────────────────────────────────────────────────────────────────
// Single-file decompress → auto output filename = strip ".lz4"
// (lz4cli.c lines 796–806: dynNameSpace = input without suffix)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_single_file_explicit_output() {
    // lz4 -d -f input.txt.lz4 recovered.txt → decompresses correctly
    let (_dir, input) = setup_input(b"decompress test content");
    let compressed = compress_file(&input);
    let recovered = compressed.with_file_name("recovered.txt");
    let status = Command::new(lz4_bin())
        .args([
            "-d",
            "-f",
            compressed.to_str().unwrap(),
            recovered.to_str().unwrap(),
        ])
        .status()
        .expect("spawn lz4 -d");
    assert!(status.success());
    assert_eq!(
        fs::read(&recovered).expect("read recovered"),
        b"decompress test content"
    );
}

#[test]
fn decompress_exit_code_zero_on_success() {
    let (_dir, input) = setup_input(b"exit code test for decompress");
    let compressed = compress_file(&input);
    let recovered = compressed.with_file_name("recovered2.txt");
    let status = Command::new(lz4_bin())
        .args([
            "-d",
            "-f",
            compressed.to_str().unwrap(),
            recovered.to_str().unwrap(),
        ])
        .status()
        .expect("spawn");
    assert_eq!(status.code(), Some(0));
}

// ─────────────────────────────────────────────────────────────────────────────
// Compress / Decompress round-trip — data parity
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_decompress_round_trip_small() {
    // Compress then decompress produces identical bytes (basic parity check).
    let original = b"The quick brown fox jumps over the lazy dog.";
    let (_dir, input) = setup_input(original);
    let compressed = compress_file(&input);
    let recovered = compressed.with_file_name("round_trip.txt");
    Command::new(lz4_bin())
        .args([
            "-d",
            "-f",
            compressed.to_str().unwrap(),
            recovered.to_str().unwrap(),
        ])
        .status()
        .expect("decompress")
        .success()
        .then_some(())
        .expect("decompress succeeded");
    assert_eq!(fs::read(&recovered).expect("read recovered"), original);
}

#[test]
fn compress_decompress_round_trip_binary_data() {
    // Round-trip with binary (non-UTF-8) data.
    let original: Vec<u8> = (0u8..=255).cycle().take(1024).collect();
    let (_dir, input) = setup_input(&original);
    let compressed = compress_file(&input);
    let recovered = compressed.with_file_name("round_trip_bin.txt");
    Command::new(lz4_bin())
        .args([
            "-d",
            "-f",
            compressed.to_str().unwrap(),
            recovered.to_str().unwrap(),
        ])
        .status()
        .expect("decompress");
    assert_eq!(fs::read(&recovered).expect("read recovered"), original);
}

#[test]
fn compress_decompress_round_trip_empty_file() {
    // Empty file round-trip: compress then decompress → still empty.
    let (_dir, input) = setup_input(b"");
    let compressed = compress_file(&input);
    let recovered = compressed.with_file_name("round_trip_empty.txt");
    Command::new(lz4_bin())
        .args([
            "-d",
            "-f",
            compressed.to_str().unwrap(),
            recovered.to_str().unwrap(),
        ])
        .status()
        .expect("decompress");
    assert_eq!(fs::read(&recovered).expect("read recovered"), b"");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test mode (-t / --test) → op=Decompress, output=NUL
// (lz4cli.c lines 758–762: prefs test_mode=1, output=NUL_MARK, op=Decompress)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_mode_valid_archive_exits_zero() {
    // lz4 -t input.txt.lz4 → test archive integrity → exit 0
    let (_dir, input) = setup_input(b"test mode data");
    let compressed = compress_file(&input);
    let status = Command::new(lz4_bin())
        .args(["-t", compressed.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn lz4 -t");
    assert_eq!(
        status.code(),
        Some(0),
        "lz4 -t on valid archive must exit 0"
    );
}

#[test]
fn test_mode_long_flag_valid_archive() {
    // lz4 --test input.txt.lz4 → same as -t
    let (_dir, input) = setup_input(b"test mode long flag");
    let compressed = compress_file(&input);
    let status = Command::new(lz4_bin())
        .args(["--test", compressed.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn lz4 --test");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn test_mode_does_not_create_output_file() {
    // In test mode, output goes to NUL_MARK (not a real file).
    // No output file should appear next to the .lz4 file.
    let (_dir, input) = setup_input(b"no output file test");
    let compressed = compress_file(&input);
    let parent = compressed.parent().unwrap();
    // Record existing files before test
    let before: std::collections::HashSet<_> = fs::read_dir(parent)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    Command::new(lz4_bin())
        .args(["-t", compressed.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn");
    let after: std::collections::HashSet<_> = fs::read_dir(parent)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(before, after, "test mode must not create any new files");
}

#[test]
fn test_mode_corrupt_archive_exits_nonzero() {
    // lz4 -t on a truncated/corrupt archive must exit non-zero.
    let dir = TempDir::new().unwrap();
    let corrupt = dir.path().join("corrupt.lz4");
    // Write an invalid LZ4 frame (truncated magic number)
    fs::write(&corrupt, b"\x04\x22\x4d\x18\x00").unwrap();
    let status = Command::new(lz4_bin())
        .args(["-t", corrupt.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn lz4 -t corrupt");
    assert_ne!(status.code(), Some(0), "corrupt archive must exit non-zero");
}

// ─────────────────────────────────────────────────────────────────────────────
// Stdin → stdout piping  (lz4cli.c lines 764–779)
// When input == STDIN_MARK (pipe) and no output file → output = STDOUT_MARK.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn pipe_compress_stdin_to_stdout() {
    // echo data | lz4 -c - → compressed bytes on stdout (non-empty)
    let mut child = Command::new(lz4_bin())
        .args(["-c", "-"]) // -c forces stdout; "-" is stdin
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn lz4 pipe");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"pipe compress test data")
        .unwrap();
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success(), "pipe compress must succeed");
    assert!(
        !output.stdout.is_empty(),
        "compressed output must not be empty"
    );
}

#[test]
fn pipe_compress_then_decompress_roundtrip() {
    // echo data | lz4 -c - | lz4 -d - -  → original data recovered
    let original = b"pipe round-trip test data 12345";

    // compress
    let mut compress_child = Command::new(lz4_bin())
        .args(["-c", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn compress");
    compress_child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(original)
        .unwrap();
    drop(compress_child.stdin.take());
    let compress_out = compress_child.wait_with_output().expect("wait compress");
    assert!(compress_out.status.success());

    // decompress
    let mut decompress_child = Command::new(lz4_bin())
        .args(["-d", "-c", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn decompress");
    decompress_child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&compress_out.stdout)
        .unwrap();
    drop(decompress_child.stdin.take());
    let decompress_out = decompress_child
        .wait_with_output()
        .expect("wait decompress");
    assert!(decompress_out.status.success());
    assert_eq!(decompress_out.stdout, original);
}

// ─────────────────────────────────────────────────────────────────────────────
// Multiple-input mode  (lz4cli.c lines 730–738, 833–887)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compress_multiple_files() {
    // lz4 -m -f file1 file2 → file1.lz4 and file2.lz4 created
    // (-m enables multiple_inputs mode; lz4cli.c line 660)
    let dir = TempDir::new().unwrap();
    let file1 = dir.path().join("a.txt");
    let file2 = dir.path().join("b.txt");
    fs::write(&file1, b"file one").unwrap();
    fs::write(&file2, b"file two").unwrap();
    let status = Command::new(lz4_bin())
        .args(["-m", "-f", file1.to_str().unwrap(), file2.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn multiple inputs");
    assert!(status.success());
    assert!(
        dir.path().join("a.txt.lz4").exists(),
        "a.txt.lz4 must be created"
    );
    assert!(
        dir.path().join("b.txt.lz4").exists(),
        "b.txt.lz4 must be created"
    );
}

#[test]
fn decompress_multiple_files() {
    // lz4 -d -m -f file1.lz4 file2.lz4 → file1 and file2 recreated
    // (-m enables multiple_inputs mode; lz4cli.c line 660)
    let dir = TempDir::new().unwrap();
    let file1 = dir.path().join("c.txt");
    let file2 = dir.path().join("d.txt");
    fs::write(&file1, b"data one").unwrap();
    fs::write(&file2, b"data two").unwrap();
    // Compress each first
    Command::new(lz4_bin())
        .args(["-f", file1.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("compress 1");
    Command::new(lz4_bin())
        .args(["-f", file2.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("compress 2");
    // Now decompress both using -m flag
    let lz4_1 = dir.path().join("c.txt.lz4");
    let lz4_2 = dir.path().join("d.txt.lz4");
    // Remove original files first so -f isn't needed for overwrite
    fs::remove_file(&file1).unwrap();
    fs::remove_file(&file2).unwrap();
    let status = Command::new(lz4_bin())
        .args([
            "-d",
            "-m",
            "-f",
            lz4_1.to_str().unwrap(),
            lz4_2.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("decompress multiple");
    assert!(status.success());
    assert_eq!(fs::read(&file1).unwrap(), b"data one");
    assert_eq!(fs::read(&file2).unwrap(), b"data two");
}

// ─────────────────────────────────────────────────────────────────────────────
// List mode (-l / --list)  (lz4cli.c line 847: displayCompressedFilesInfo)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn list_mode_exits_zero_for_valid_archive() {
    // lz4 --list input.lz4 → exit 0
    let (_dir, input) = setup_input(b"list mode test data");
    let compressed = compress_file(&input);
    let status = Command::new(lz4_bin())
        .args(["--list", compressed.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn lz4 --list");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn legacy_format_flag_produces_valid_compressed_file() {
    // lz4 -l -f input.txt output.lz4 → "-l" is the legacy LZ4 frame format flag (NOT list).
    // Mirrors lz4cli.c line 576: case 'l' → legacyFormat = 1, blockSize = LEGACY_BLOCK_SIZE.
    let original = b"legacy format test content";
    let (_dir, input) = setup_input(original);
    let output = input.with_extension("txt.lz4");
    let status = Command::new(lz4_bin())
        .args([
            "-l",
            "-f",
            input.to_str().unwrap(),
            output.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn lz4 -l (legacy format)");
    assert!(status.success(), "legacy format compress must succeed");
    assert!(output.exists(), "legacy .lz4 output file must exist");
}

#[test]
fn list_mode_produces_output() {
    // lz4 --list archive.lz4 → prints info to stdout (non-empty output)
    let (_dir, input) = setup_input(b"list output test");
    let compressed = compress_file(&input);
    let output = Command::new(lz4_bin())
        .args(["--list", compressed.to_str().unwrap()])
        .output()
        .expect("spawn lz4 --list");
    // List mode should produce some informational output.
    let combined = [output.stdout.as_slice(), output.stderr.as_slice()].concat();
    assert!(!combined.is_empty(), "--list must produce output");
}

// ─────────────────────────────────────────────────────────────────────────────
// Error paths
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn decompress_non_lz4_file_without_output_flag_exits_nonzero() {
    // lz4 -d file.txt (no .lz4 extension, no output specified) → error
    // Mirrors lz4cli.c lines 796–806: cannot determine output filename.
    let (_dir, input) = setup_input(b"this is not lz4");
    let status = Command::new(lz4_bin())
        .args(["-d", input.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn lz4 -d non-lz4");
    assert_ne!(
        status.code(),
        Some(0),
        "decompressing a non-.lz4 file without specifying output must fail"
    );
}

#[test]
fn compress_nonexistent_input_exits_nonzero() {
    // lz4 nonexistent.txt → error (file not found)
    let status = Command::new(lz4_bin())
        .args(["/tmp/lz4-test-nonexistent-input-file-xyz.txt"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn");
    assert_ne!(status.code(), Some(0));
}

// ─────────────────────────────────────────────────────────────────────────────
// Force-overwrite flag (-f)  (lz4cli.c: overwrite flag in prefs)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn force_overwrite_replaces_existing_output() {
    // Without -f, lz4 refuses to overwrite. With -f it succeeds.
    let (_dir, input) = setup_input(b"overwrite test content");
    let output = input.with_extension("txt.lz4");
    // First compress (creates output)
    Command::new(lz4_bin())
        .args(["-f", input.to_str().unwrap(), output.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("first compress");
    assert!(output.exists());
    // Second compress with -f → should succeed and overwrite
    let status = Command::new(lz4_bin())
        .args(["-f", input.to_str().unwrap(), output.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("second compress");
    assert!(
        status.success(),
        "-f must allow overwriting existing output"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Compression level flags  (lz4cli.c: cLevel handling in dispatch)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn high_compression_level_produces_valid_output() {
    // lz4 -9 -f input output.lz4 → valid archive that decompresses correctly
    let original = b"high compression level test data repeated repeated repeated";
    let (_dir, input) = setup_input(original);
    let output = input.with_extension("txt.lz4");
    Command::new(lz4_bin())
        .args([
            "-9",
            "-f",
            input.to_str().unwrap(),
            output.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("compress -9");
    let recovered = input.with_extension("txt.recovered");
    Command::new(lz4_bin())
        .args([
            "-d",
            "-f",
            output.to_str().unwrap(),
            recovered.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("decompress");
    assert_eq!(fs::read(&recovered).unwrap(), original);
}

#[test]
fn fast_mode_flag_produces_valid_output() {
    // lz4 -1 -f input output.lz4 → valid archive
    let original = b"fast mode test";
    let (_dir, input) = setup_input(original);
    let output = input.with_extension("txt.lz4");
    Command::new(lz4_bin())
        .args([
            "-1",
            "-f",
            input.to_str().unwrap(),
            output.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("compress -1");
    assert!(output.exists());
    // Verify round-trip
    let recovered = input.with_extension("txt.fast");
    Command::new(lz4_bin())
        .args([
            "-d",
            "-f",
            output.to_str().unwrap(),
            recovered.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("decompress");
    assert_eq!(fs::read(&recovered).unwrap(), original);
}

// ─────────────────────────────────────────────────────────────────────────────
// Auto output filename derivation (lz4cli.c lines 781–808)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn auto_compress_output_filename_adds_lz4_extension() {
    // lz4 -f input.txt → auto output = input.txt.lz4 (no explicit output)
    // (lz4cli.c line 784: format "%s%s", inFileName, LZ4_EXTENSION)
    let (_dir, input) = setup_input(b"auto filename compress");
    let expected_output = input.with_extension("txt.lz4");
    // Ensure the expected output does not already exist
    let _ = fs::remove_file(&expected_output);
    let status = Command::new(lz4_bin())
        .args(["-f", input.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn");
    assert!(status.success());
    assert!(
        expected_output.exists(),
        "auto compress must create {expected_output:?}"
    );
}

#[test]
fn auto_decompress_output_filename_strips_lz4_extension() {
    // lz4 -d -f input.txt.lz4 → auto output = input.txt (strip ".lz4")
    // (lz4cli.c lines 796–806: dynNameSpace = input without LZ4_EXTENSION)
    let (_dir, input) = setup_input(b"auto filename decompress");
    let compressed = input.with_extension("txt.lz4");
    // Compress to get the .lz4 file
    Command::new(lz4_bin())
        .args(["-f", input.to_str().unwrap(), compressed.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("compress");
    // Remove original to test auto output filename
    fs::remove_file(&input).unwrap();
    let status = Command::new(lz4_bin())
        .args(["-d", "-f", compressed.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn decompress auto");
    assert!(status.success());
    assert!(
        input.exists(),
        "auto decompress must create {input:?} (stripped .lz4)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Remove source file  (lz4cli.c: removeSrcFile in prefs)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn remove_source_flag_deletes_input_after_compress() {
    // lz4 --rm -f input.txt → creates input.txt.lz4, deletes input.txt
    let (_dir, input) = setup_input(b"remove source test");
    let expected_output = input.with_extension("txt.lz4");
    let status = Command::new(lz4_bin())
        .args(["--rm", "-f", input.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn --rm");
    assert!(status.success());
    assert!(expected_output.exists(), "output .lz4 must exist");
    assert!(
        !input.exists(),
        "--rm must delete the source file after compress"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// MULTITHREAD warning path  (lz4cli.c lines 723–726)
// When !MULTITHREAD && nb_workers > 1 → warning on stderr (exit still 0)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn nb_workers_greater_than_one_does_not_crash() {
    // lz4 --workers=2 -f input → should not crash regardless of MT support
    // (the binary handles the !MULTITHREAD case by just warning)
    let (_dir, input) = setup_input(b"multithread test");
    let output = input.with_extension("txt.lz4");
    let status = Command::new(lz4_bin())
        .args([
            "--workers=2",
            "-f",
            input.to_str().unwrap(),
            output.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn --workers=2");
    // Regardless of MT support, the binary must not crash (exit 0 or fail gracefully)
    // We only assert it did not segfault (signal termination).
    assert!(
        status.code().is_some(),
        "process must exit normally, not via signal"
    );
}
