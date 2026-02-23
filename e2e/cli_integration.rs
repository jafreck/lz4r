// e2e/suite_09_cli_integration.rs — CLI integration tests (Suite 09)
//
// Tests the `lz4` binary as a black-box CLI tool using std::process::Command.
// Covers argument parsing, compress/decompress dispatch, exit codes, test mode,
// and list mode.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

/// Locate the `lz4` binary produced by Cargo.
fn lz4_bin() -> PathBuf {
    // CARGO_BIN_EXE_lz4 is set by Cargo when running integration tests.
    // Fall back to walking up from the test binary location.
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_lz4") {
        return PathBuf::from(p);
    }
    let mut p = std::env::current_exe().unwrap();
    p.pop(); // remove test binary filename
    if p.ends_with("deps") {
        p.pop();
    }
    p.push("lz4");
    p
}

/// Create a TempDir containing a text file with ~4 KB of content.
fn make_temp_input() -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let input_path = dir.path().join("input.txt");
    let content = "Hello, LZ4!\n".repeat(341); // ~4 KB
    fs::write(&input_path, content).unwrap();
    (dir, input_path)
}

// ── 1. Compress / decompress roundtrip ───────────────────────────────────────

#[test]
fn test_cli_compress_decompress_roundtrip() {
    let (dir, input) = make_temp_input();
    let original = fs::read(&input).unwrap();

    let compressed = dir.path().join("output.lz4");
    let roundtrip = dir.path().join("roundtrip.txt");

    // Compress
    let status = Command::new(lz4_bin())
        .args(["-f", input.to_str().unwrap(), compressed.to_str().unwrap()])
        .current_dir(dir.path())
        .status()
        .expect("failed to run lz4 compress");
    assert!(status.success(), "compress step should exit 0");
    assert!(compressed.exists(), "compressed file should exist");

    // Decompress
    let status = Command::new(lz4_bin())
        .args([
            "-d",
            "-f",
            compressed.to_str().unwrap(),
            roundtrip.to_str().unwrap(),
        ])
        .current_dir(dir.path())
        .status()
        .expect("failed to run lz4 decompress");
    assert!(status.success(), "decompress step should exit 0");

    let recovered = fs::read(&roundtrip).unwrap();
    assert_eq!(original, recovered, "roundtrip output must match original");
}

// ── 2. --version ──────────────────────────────────────────────────────────────

#[test]
fn test_cli_version() {
    let output = Command::new(lz4_bin())
        .arg("--version")
        .output()
        .expect("failed to run lz4 --version");

    assert!(
        output.status.success(),
        "--version should exit 0; status: {}",
        output.status
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("1.10.0"),
        "--version stdout should contain '1.10.0'; got: {stdout}"
    );
}

// ── 3. --help ─────────────────────────────────────────────────────────────────

#[test]
fn test_cli_help() {
    let output = Command::new(lz4_bin())
        .arg("--help")
        .output()
        .expect("failed to run lz4 --help");

    assert!(
        output.status.success(),
        "--help should exit 0; status: {}",
        output.status
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.to_lowercase().contains("usage"),
        "--help output should contain 'usage'; got: {combined}"
    );
}

// ── 4. -k keeps source file ───────────────────────────────────────────────────

#[test]
fn test_cli_keep_source_file() {
    let (dir, input) = make_temp_input();
    let compressed = dir.path().join("kept_output.lz4");

    let status = Command::new(lz4_bin())
        .args([
            "-k",
            "-f",
            input.to_str().unwrap(),
            compressed.to_str().unwrap(),
        ])
        .current_dir(dir.path())
        .status()
        .expect("failed to run lz4 -k");

    assert!(status.success(), "-k compress should exit 0");
    assert!(
        input.exists(),
        "source file must still exist after -k compress"
    );
    assert!(compressed.exists(), "compressed output must exist");
}

// ── 5. -d decompress flag ─────────────────────────────────────────────────────

#[test]
fn test_cli_decompress_flag() {
    let (dir, input) = make_temp_input();
    let original = fs::read(&input).unwrap();

    // First compress so we have something to decompress.
    let compressed = dir.path().join("to_decompress.lz4");
    Command::new(lz4_bin())
        .args(["-f", input.to_str().unwrap(), compressed.to_str().unwrap()])
        .current_dir(dir.path())
        .status()
        .expect("compress step failed");

    let decompressed = dir.path().join("decompressed.txt");
    let status = Command::new(lz4_bin())
        .args([
            "-d",
            "-f",
            compressed.to_str().unwrap(),
            decompressed.to_str().unwrap(),
        ])
        .current_dir(dir.path())
        .status()
        .expect("failed to run lz4 -d");

    assert!(status.success(), "-d decompress should exit 0");
    let recovered = fs::read(&decompressed).unwrap();
    assert_eq!(original, recovered, "decompressed data must match original");
}

// ── 6. -t test mode on valid .lz4 ────────────────────────────────────────────

#[test]
fn test_cli_test_mode_valid() {
    let (dir, input) = make_temp_input();
    let compressed = dir.path().join("valid_test.lz4");

    Command::new(lz4_bin())
        .args(["-f", input.to_str().unwrap(), compressed.to_str().unwrap()])
        .current_dir(dir.path())
        .status()
        .expect("compress step failed");

    let status = Command::new(lz4_bin())
        .args(["-t", compressed.to_str().unwrap()])
        .current_dir(dir.path())
        .status()
        .expect("failed to run lz4 -t on valid file");

    assert!(status.success(), "-t on a valid .lz4 file should exit 0");
}

// ── 7. -t test mode on corrupted .lz4 ────────────────────────────────────────

#[test]
fn test_cli_test_mode_corrupt() {
    let dir = TempDir::new().unwrap();
    let corrupt = dir.path().join("corrupt.lz4");

    // Valid LZ4 frame magic followed by garbage bytes.
    let mut data = vec![0x04u8, 0x22, 0x4D, 0x18];
    data.extend_from_slice(&[0xFF; 64]);
    let mut f = fs::File::create(&corrupt).unwrap();
    f.write_all(&data).unwrap();

    let status = Command::new(lz4_bin())
        .args(["-t", corrupt.to_str().unwrap()])
        .current_dir(dir.path())
        .status()
        .expect("failed to run lz4 -t on corrupt file");

    assert!(
        !status.success(),
        "-t on a corrupt .lz4 file should exit non-zero"
    );
}

// ── 8. Non-existent input ─────────────────────────────────────────────────────

#[test]
fn test_cli_nonexistent_input() {
    let dir = TempDir::new().unwrap();
    let status = Command::new(lz4_bin())
        .args(["/nonexistent_path_abc123_lz4test", "/tmp/out_lz4_test.lz4"])
        .current_dir(dir.path())
        .status()
        .expect("failed to run lz4 with nonexistent input");

    assert!(
        !status.success(),
        "lz4 with nonexistent input should exit non-zero"
    );
}

// ── 9. --list mode ────────────────────────────────────────────────────────────

#[test]
fn test_cli_list_mode() {
    let (dir, input) = make_temp_input();
    let compressed = dir.path().join("list_test.lz4");

    Command::new(lz4_bin())
        .args(["-f", input.to_str().unwrap(), compressed.to_str().unwrap()])
        .current_dir(dir.path())
        .status()
        .expect("compress step failed");

    let output = Command::new(lz4_bin())
        .args(["--list", compressed.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .expect("failed to run lz4 --list");

    assert!(
        output.status.success(),
        "--list on valid .lz4 file should exit 0; status: {}",
        output.status
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // --list should print some file information to stdout.
    assert!(
        !stdout.trim().is_empty(),
        "--list stdout should contain file info; got empty output"
    );
}
