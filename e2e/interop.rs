//! Interoperability tests — Gap 1 from correctness-and-benchmark-plan
//!
//! These 6 tests prove byte-compatible output between this Rust implementation
//! and the reference C `lz4` binary.  If the system `lz4` binary is not found
//! the test prints a skip message and returns without failing — no `#[ignore]`
//! is used so the tests always appear in the test count.

extern crate lz4;

use lz4::frame::{
    compress_frame_to_vec, decompress_frame_to_vec, lz4f_compress_frame, lz4f_compress_frame_bound,
    ContentChecksum, FrameInfo, Preferences,
};
use std::io::Write;
use std::process::Command;
use tempfile::NamedTempFile;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Returns the path to the system C `lz4` binary, or `None` if not found.
fn system_lz4() -> Option<String> {
    // Allow override via environment variable.
    if let Ok(p) = std::env::var("LZ4_BIN") {
        if std::path::Path::new(&p).exists() {
            return Some(p);
        }
    }
    let out = Command::new("which").arg("lz4").output().ok()?;
    if out.status.success() {
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !path.is_empty() {
            return Some(path);
        }
    }
    None
}

/// Path to our Rust `lz4` binary (set by Cargo at compile time).
fn rust_lz4() -> &'static str {
    env!("CARGO_BIN_EXE_lz4")
}

/// Load the 64 KiB enwik8 fixture.
fn fixture() -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/enwik8_64k.bin");
    std::fs::read(path)
        .expect("tests/fixtures/enwik8_64k.bin must exist — run the fixture setup step")
}

/// Write `data` into a new temporary file and return it (keeps the file alive).
fn write_tmp(data: &[u8]) -> NamedTempFile {
    let mut f = NamedTempFile::new().expect("create temp file");
    f.write_all(data).expect("write temp file");
    f.flush().expect("flush temp file");
    f
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 1 — Rust frame compress → C decompress
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn rust_frame_compress_c_decompress() {
    let lz4_bin = match system_lz4() {
        Some(p) => p,
        None => {
            println!("SKIP rust_frame_compress_c_decompress: system lz4 binary not found");
            return;
        }
    };

    let original = fixture();

    // Compress with Rust.
    let compressed = compress_frame_to_vec(&original);
    assert!(
        !compressed.is_empty(),
        "rust compression produced empty output"
    );

    // Write compressed bytes to a temp file.
    let compressed_file = write_tmp(&compressed);
    let output_file = NamedTempFile::new().expect("create output temp file");

    // Decompress with C lz4.
    let status = Command::new(&lz4_bin)
        .args([
            "-d",
            "-f",
            compressed_file.path().to_str().unwrap(),
            output_file.path().to_str().unwrap(),
        ])
        .status()
        .expect("spawn system lz4");

    assert!(status.success(), "system lz4 -d failed: {:?}", status);

    let decompressed = std::fs::read(output_file.path()).expect("read decompressed output");
    assert_eq!(
        decompressed, original,
        "C-decompressed bytes differ from original"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2 — C frame compress → Rust decompress
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn c_frame_compress_rust_decompress() {
    let lz4_bin = match system_lz4() {
        Some(p) => p,
        None => {
            println!("SKIP c_frame_compress_rust_decompress: system lz4 binary not found");
            return;
        }
    };

    let original = fixture();
    let input_file = write_tmp(&original);
    let compressed_file = NamedTempFile::new().expect("create compressed temp file");

    // Compress with C lz4.
    let status = Command::new(&lz4_bin)
        .args([
            "-f",
            input_file.path().to_str().unwrap(),
            compressed_file.path().to_str().unwrap(),
        ])
        .status()
        .expect("spawn system lz4");

    assert!(status.success(), "system lz4 compress failed: {:?}", status);

    let compressed = std::fs::read(compressed_file.path()).expect("read compressed file");

    // Decompress with Rust.
    let decompressed =
        decompress_frame_to_vec(&compressed).expect("rust frame decompression failed");

    assert_eq!(
        decompressed, original,
        "Rust-decompressed bytes differ from original"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 3 — Rust CLI compress → C decompress
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn rust_cli_compress_c_decompress() {
    let lz4_bin = match system_lz4() {
        Some(p) => p,
        None => {
            println!("SKIP rust_cli_compress_c_decompress: system lz4 binary not found");
            return;
        }
    };

    let original = fixture();
    let input_file = write_tmp(&original);
    let compressed_file = NamedTempFile::new().expect("create compressed temp file");
    let output_file = NamedTempFile::new().expect("create output temp file");

    // Compress with our Rust lz4 binary.
    let status = Command::new(rust_lz4())
        .args([
            "-f",
            input_file.path().to_str().unwrap(),
            compressed_file.path().to_str().unwrap(),
        ])
        .status()
        .expect("spawn rust lz4");

    assert!(status.success(), "rust lz4 compress failed: {:?}", status);

    // Decompress with the system C lz4.
    let status = Command::new(&lz4_bin)
        .args([
            "-d",
            "-f",
            compressed_file.path().to_str().unwrap(),
            output_file.path().to_str().unwrap(),
        ])
        .status()
        .expect("spawn system lz4");

    assert!(status.success(), "system lz4 -d failed: {:?}", status);

    let decompressed = std::fs::read(output_file.path()).expect("read decompressed file");
    assert_eq!(
        decompressed, original,
        "C-decompressed Rust-compressed bytes differ"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 4 — C compress → Rust CLI decompress
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn c_compress_rust_cli_decompress() {
    let lz4_bin = match system_lz4() {
        Some(p) => p,
        None => {
            println!("SKIP c_compress_rust_cli_decompress: system lz4 binary not found");
            return;
        }
    };

    let original = fixture();
    let input_file = write_tmp(&original);
    let compressed_file = NamedTempFile::new().expect("create compressed temp file");
    let output_file = NamedTempFile::new().expect("create output temp file");

    // Compress with the system C lz4.
    let status = Command::new(&lz4_bin)
        .args([
            "-f",
            input_file.path().to_str().unwrap(),
            compressed_file.path().to_str().unwrap(),
        ])
        .status()
        .expect("spawn system lz4");

    assert!(status.success(), "system lz4 compress failed: {:?}", status);

    // Decompress with our Rust lz4 binary.
    let status = Command::new(rust_lz4())
        .args([
            "-d",
            "-f",
            compressed_file.path().to_str().unwrap(),
            output_file.path().to_str().unwrap(),
        ])
        .status()
        .expect("spawn rust lz4");

    assert!(status.success(), "rust lz4 -d failed: {:?}", status);

    let decompressed = std::fs::read(output_file.path()).expect("read decompressed file");
    assert_eq!(
        decompressed, original,
        "Rust-decompressed C-compressed bytes differ"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 5 — Content checksum: bit flip detected by C decompressor
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn frame_content_checksum_bit_flip() {
    let lz4_bin = match system_lz4() {
        Some(p) => p,
        None => {
            println!("SKIP frame_content_checksum_bit_flip: system lz4 binary not found");
            return;
        }
    };

    let original = fixture();

    // Compress with content checksum enabled.
    let prefs = Preferences {
        frame_info: FrameInfo {
            content_checksum_flag: ContentChecksum::Enabled,
            ..Default::default()
        },
        ..Default::default()
    };
    let bound = lz4f_compress_frame_bound(original.len(), Some(&prefs));
    let mut compressed = vec![0u8; bound];
    let compressed_size = lz4f_compress_frame(&mut compressed, &original, Some(&prefs))
        .expect("compression with checksum should succeed");
    compressed.truncate(compressed_size);

    // Flip a byte roughly in the middle of the compressed payload (skip the
    // 7-byte minimum frame header to avoid corrupting the header in a way that
    // is detected before the checksum is read — we want checksum detection).
    let flip_pos = compressed_size / 2;
    assert!(
        flip_pos > 7,
        "compressed output too small for mid-payload flip"
    );
    compressed[flip_pos] ^= 0xFF;

    let corrupted_file = write_tmp(&compressed);
    let output_file = NamedTempFile::new().expect("create output temp file");

    // The system lz4 should detect the corruption and exit non-zero.
    let status = Command::new(&lz4_bin)
        .args([
            "-d",
            "-f",
            corrupted_file.path().to_str().unwrap(),
            output_file.path().to_str().unwrap(),
        ])
        .status()
        .expect("spawn system lz4");

    assert!(
        !status.success(),
        "system lz4 should have detected corruption but exited successfully"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 6 — Synthetic corpus: Rust → C → Rust full roundtrip
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn synthetic_corpus_roundtrip() {
    let lz4_bin = match system_lz4() {
        Some(p) => p,
        None => {
            println!("SKIP synthetic_corpus_roundtrip: system lz4 binary not found");
            return;
        }
    };

    let original = fixture();

    // Step 1: Rust compress.
    let compressed = compress_frame_to_vec(&original);
    assert!(
        !compressed.is_empty(),
        "rust compression produced empty output"
    );

    // Step 2: C decompress → temp file.
    let compressed_file = write_tmp(&compressed);
    let c_decompressed_file = NamedTempFile::new().expect("create c-decompressed temp file");

    let status = Command::new(&lz4_bin)
        .args([
            "-d",
            "-f",
            compressed_file.path().to_str().unwrap(),
            c_decompressed_file.path().to_str().unwrap(),
        ])
        .status()
        .expect("spawn system lz4");

    assert!(status.success(), "system lz4 -d failed: {:?}", status);

    // Step 3: Re-compress the C-decompressed data with C lz4.
    let re_compressed_file = NamedTempFile::new().expect("create re-compressed temp file");

    let status = Command::new(&lz4_bin)
        .args([
            "-f",
            c_decompressed_file.path().to_str().unwrap(),
            re_compressed_file.path().to_str().unwrap(),
        ])
        .status()
        .expect("spawn system lz4");

    assert!(
        status.success(),
        "system lz4 re-compress failed: {:?}",
        status
    );

    // Step 4: Rust decompress the C re-compressed data.
    let re_compressed = std::fs::read(re_compressed_file.path()).expect("read re-compressed file");
    let final_output = decompress_frame_to_vec(&re_compressed)
        .expect("rust decompression of C-compressed data failed");

    assert_eq!(
        final_output, original,
        "full Rust→C→Rust roundtrip produced different bytes"
    );
}
