# Developer Guide: lz4-rust

**Migration**: lz4-to-rust  
**Source**: LZ4 v1.10.0 (C)  
**Target**: Rust (stable, edition 2021)  
**Date**: 2026-02-22  
**Version**: 1.10.0

---

## Prerequisites

| Tool | Version | Purpose |
|------|---------|---------|
| Rust toolchain | stable ≥ 1.70 | Compilation |
| Cargo | (bundled with Rust) | Build, test, dependency management |
| `rustup` | any | Toolchain management |

Install Rust: https://rustup.rs

No C compiler or system LZ4 library is required — this is a pure-Rust implementation.

---

## Getting Started

### Clone / Locate the Crate

The migrated crate lives at:
```
tmp/lz4-rust-output/
```
(relative to the lz4-c-project fixture root).

### Build

```bash
cd tmp/lz4-rust-output
cargo build
```

For an optimized release build:
```bash
cargo build --release
```

### Run Tests

```bash
cargo test
```

Expected output: **856 tests pass** across 23 integration test suites + 2 doc-tests, 0 failures.

### Check (no codegen)

```bash
cargo check
```

### Lint

```bash
cargo clippy
```

Note: 16 known warnings (see [Known Issues](./known-issues.md)) are expected and do not indicate bugs.

---

## Project Layout

```
tmp/lz4-rust-output/
├── Cargo.toml          # Package manifest
├── Cargo.lock          # Locked dependency versions
├── src/
│   ├── lib.rs          # Crate root — public re-exports
│   ├── block/          # Core block compression/decompression
│   ├── hc/             # High-compression codec
│   ├── frame/          # LZ4 Frame format
│   ├── file.rs         # File I/O wrapper
│   └── xxhash.rs       # xxhash-rust crate wrapper
├── tests/              # Integration tests (one file per migration task)
└── __tests__/          # Additional test fixtures
```

---

## Dependencies

```toml
[dependencies]
xxhash-rust = { version = "0.8", features = ["xxh32"] }
```

Only one external dependency. `xxhash-rust` provides a pure-Rust implementation of XXH32/XXH64. All other functionality is implemented in this crate using the Rust standard library.

### Updating Dependencies

```bash
cargo update
```

Dependency versions are locked in `Cargo.lock`. The `xxhash-rust` crate is semantically versioned and backward-compatible within the `0.8` minor series.

---

## Using the Crate

### As a Library

Add to your `Cargo.toml`:
```toml
[dependencies]
lz4 = { path = "path/to/tmp/lz4-rust-output" }
```

### One-Shot Block Compression

```rust
use lz4::{lz4_compress_default, lz4_decompress_safe, compress_bound};

fn compress_example(data: &[u8]) -> Vec<u8> {
    let max_size = compress_bound(data.len() as i32) as usize;
    let mut compressed = vec![0u8; max_size];
    let n = lz4_compress_default(data, &mut compressed).expect("compression failed");
    compressed.truncate(n);
    compressed
}

fn decompress_example(compressed: &[u8], original_size: usize) -> Vec<u8> {
    let mut output = vec![0u8; original_size];
    let n = lz4_decompress_safe(compressed, &mut output).expect("decompression failed");
    output.truncate(n);
    output
}
```

### Streaming Block Compression

```rust
use lz4::{Lz4Stream, compress_bound};

let mut stream = lz4::Lz4Stream::create();
for block in input_blocks {
    let bound = compress_bound(block.len() as i32) as usize;
    let mut out = vec![0u8; bound];
    let n = stream.compress_fast_continue(block, &mut out, 1)
        .expect("streaming compress failed");
    // send/store out[..n]
}
```

### Frame Compression (one-shot)

```rust
use lz4::frame::compress::{lz4f_compress_frame, lz4f_compress_bound};

let bound = lz4f_compress_bound(data.len(), None);
let mut frame = vec![0u8; bound];
let n = lz4f_compress_frame(&mut frame, data, None)?;
frame.truncate(n);
```

### Frame Decompression

```rust
use lz4::frame::decompress::{lz4f_create_decompression_context, lz4f_decompress, lz4f_free_decompression_context};
use lz4::frame::types::LZ4F_VERSION;

let mut ctx = lz4f_create_decompression_context(LZ4F_VERSION)?;
let mut output = vec![0u8; 4 * 1024 * 1024];
let (src_consumed, dst_written) = lz4f_decompress(&mut ctx, &mut output, &frame, None)?;
output.truncate(dst_written);
lz4f_free_decompression_context(ctx);
```

### File I/O (streaming)

```rust
use std::io::{Read, Write};
use lz4::file::{Lz4ReadFile, Lz4WriteFile};

// Compress to a file
let file = std::fs::File::create("output.lz4")?;
let mut writer = Lz4WriteFile::open(file, None)?;
writer.write_all(data)?;
let inner_file = writer.close()?;  // finalizes frame

// Decompress from a file
let file = std::fs::File::open("output.lz4")?;
let mut reader = Lz4ReadFile::open(file)?;
let mut decompressed = Vec::new();
reader.read_to_end(&mut decompressed)?;
```

---

## Testing

### Test Organization

Integration tests are in `tests/`, one file per migration task:

| Test File | Coverage |
|-----------|---------|
| `task_001_crate_setup.rs` | Cargo project structure |
| `task_002_xxhash.rs` | XXH32 correctness |
| `task_003_block_types.rs` | Constants, helpers |
| `task_004_compress.rs` | Block compression |
| `task_005_stream.rs` | Streaming compress |
| `task_006_decompress_core.rs` | Decompress engine |
| `task_007_decompress_api.rs` | Decompress API |
| `task_008_hc_types.rs` | HC context init |
| `task_009_hc_encode.rs` | Sequence encoder |
| `task_010_hc_lz4mid.rs` | LZ4MID strategy |
| `task_011_hc_search.rs` | HC search |
| `task_012_compress_hc.rs` | HC compress loop |
| `task_013_dispatch.rs` | Strategy dispatch |
| `task_014_hc_api.rs` | Public HC API |
| `task_015_frame_types.rs` | Frame types |
| `task_016_frame_header.rs` | Frame header encode/decode |
| `task_017_cdict.rs` | Dictionary API |
| `task_018_frame_compress.rs` | Frame compression |
| `task_019_frame_decompress.rs` | Frame decompression |
| `task_020_file.rs` | File I/O |
| `task_021_lib.rs` | Crate root + integration |

### Running a Specific Test

```bash
cargo test task_004_compress
```

### Running Tests with Output

```bash
cargo test -- --nocapture
```

---

## Development Workflow

### Modifying a Module

1. Edit the relevant `src/` file.
2. Run `cargo check` to verify compilation.
3. Run `cargo test` to verify correctness.
4. Optionally run `cargo clippy` for linting.

### Adding a Function

All public functions should:
1. Be declared `pub` in the appropriate module.
2. Have a Rustdoc comment (`///`) describing parameters, return value, and safety (if `unsafe`).
3. Be re-exported from `src/lib.rs` if they are top-level API items.
4. Have at least one integration test.

### Unsafe Code

All `unsafe` blocks **must** include a `// SAFETY:` comment explaining the invariant that makes the operation safe. The `decompress_core.rs` module is especially safety-critical — all C-side bounds checks must be preserved.

---

## Build Outputs

| Build mode | Output |
|-----------|--------|
| `cargo build` | `target/debug/liblz4.rlib` (Rust lib), `target/debug/liblz4.so` / `.dylib` / `.dll` (C-compatible dynamic lib) |
| `cargo build --release` | `target/release/liblz4.rlib`, `target/release/liblz4.so` |

The crate-type `["cdylib", "rlib"]` matches the C library's dual static/shared output.

---

## Configuration

The library does not use any environment variables or configuration files at runtime. All tuning is done at compile time via Rust constants in `src/block/types.rs` and `src/hc/types.rs`.

To change hash table size or other parameters, edit the relevant constants and rebuild.

---

## See Also

- [Architecture Guide](./architecture-guide.md)
- [API Reference](./api-reference.md)
- [Migration Summary](./migration-summary.md)
- [Known Issues](./known-issues.md)
