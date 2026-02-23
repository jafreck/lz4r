# lz4r

[![CI](https://github.com/jafreck/lz4r/actions/workflows/ci.yml/badge.svg)](https://github.com/jafreck/lz4r/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/jafreck/lz4r/branch/main/graph/badge.svg)](https://codecov.io/gh/jafreck/lz4r)
[![Crates.io](https://img.shields.io/crates/v/lz4r.svg)](https://crates.io/crates/lz4r)
[![docs.rs](https://img.shields.io/docsrs/lz4r)](https://docs.rs/lz4r)
[![License: GPL-2.0](https://img.shields.io/badge/license-GPL--2.0-blue.svg)](LICENSE)

Pure-Rust port of the [LZ4 compression library](https://github.com/lz4/lz4) (v1.10.0), providing the full LZ4 C API surface — block compression, high-compression mode, streaming frame format, and file I/O — with no C dependencies.

Compressed output is **bit-for-bit identical** to the reference C implementation across all compression modes and levels.

---

## Related Projects

| Project | Description |
|---------|-------------|
| [lz4/lz4](https://github.com/lz4/lz4) | Original C implementation — the upstream reference this crate ports |
| [inikep/lzbench](https://github.com/inikep/lzbench) | In-memory benchmark harness used for apples-to-apples throughput comparison |
| [jafreck/AAMF](https://github.com/jafreck/AAMF) | Automated Architecture Migration Framework — the AI-assisted toolchain that performed the primary migration work |

---

## Features

- **Block API** — one-shot `compress_default`, `compress_fast`, `decompress_safe`, and partial decompression
- **High-Compression (HC)** — `compress_hc` with configurable compression levels 1–12
- **Frame API** — `LZ4F`-prefixed streaming compress/decompress with content checksums, dictionary support, and auto-flush
- **File I/O** — `Lz4ReadFile` / `Lz4WriteFile` wrappers for `std::io::{Read, Write}`
- **C ABI shim** — optional `c-abi` feature exports `LZ4_compress_default`, `LZ4_compress_fast`, `LZ4_decompress_safe`, and `LZ4_compress_HC` as a `staticlib` for drop-in use with C consumers (e.g. lzbench)
- **Multi-threaded I/O** — optional `multithread` feature mirrors the `LZ4IO_MULTITHREAD` path from the C programs

---

## Usage

```toml
[dependencies]
lz4 = "1.10.0"
```

### Block compression

```rust
use lz4::{compress_default, decompress_safe};

let input = b"hello world hello world hello world";
let bound = lz4::compress_bound(input.len() as i32) as usize;
let mut compressed = vec![0u8; bound];

let compressed_size = compress_default(input, &mut compressed).unwrap();
compressed.truncate(compressed_size as usize);

let mut output = vec![0u8; input.len()];
let n = decompress_safe(&compressed, &mut output).unwrap();
assert_eq!(&output[..n as usize], &input[..]);
```

### High-compression block

```rust
use lz4::compress_hc;

let input = b"highly compressible text content …";
let bound = lz4::compress_bound(input.len() as i32) as usize;
let mut compressed = vec![0u8; bound];

// Levels 1–12; 9 is a good balance of ratio vs speed
let n = compress_hc(input, &mut compressed, 9).unwrap();
```

### Frame streaming

```rust
use lz4::frame::{Lz4FCompressContext, Lz4FDecompressContext, Preferences};

let prefs = Preferences::default();
let mut ctx = Lz4FCompressContext::new()?;
// … write chunks via ctx.compress_update(…)
```

---

## Building

```bash
# Debug build
cargo build

# Optimised release build
cargo build --release

# With multi-threaded I/O support
cargo build --release --features multithread

# As a C-compatible static library (for lzbench integration)
RUSTFLAGS="-C panic=abort" cargo build --release --features c-abi
# → target/release/liblz4.a
```

---

## Testing

```bash
cargo test
```

All 856 tests across 23 integration test suites and 2 doc-tests are expected to pass.

```bash
# Fuzz targets (requires cargo-fuzz + nightly)
cargo +nightly fuzz run block_roundtrip
cargo +nightly fuzz run frame_roundtrip
cargo +nightly fuzz run decompress_block_arbitrary
cargo +nightly fuzz run decompress_frame_arbitrary
```

---

## Benchmarks

### In-crate microbenchmarks (Criterion)

```bash
cargo bench
```

Results are written to `target/criterion/`. HTML reports are available at
`target/criterion/report/index.html`.

### Apples-to-apples: lzbench (Rust vs C)

The definitive throughput comparison uses the [lzbench](https://github.com/inikep/lzbench)
harness to run **both the C reference and this Rust port through the identical timing
loop**, eliminating harness artefacts.

#### Methodology

`liblz4.a` is built with `--features c-abi` and linked into a patched lzbench binary
(`lzbench-rust`) that replaces `lz4.o` / `lz4hc.o` with the Rust archive. The four
C-ABI symbols (`LZ4_compress_default`, `LZ4_compress_fast`, `LZ4_decompress_safe`,
`LZ4_compress_HC`) are exported as `#[no_mangle] pub unsafe extern "C"` shims that
forward to the native Rust block and HC APIs.

Environment: lzbench 2.2.1 | Clang 17 | 64-bit macOS (Apple Silicon) | Silesia corpus

#### Compression throughput summary (MB/s) — `lz4` default

| File | C | Rust | Δ |
|------|--:|-----:|--:|
| webster | 613 | 569 | −7% |
| mozilla | 861 | 803 | −7% |
| mr | 865 | 815 | −6% |
| dickens | 534 | 509 | −5% |
| x-ray | 2706 | 2313 | −15% |
| ooffice | 814 | 729 | −10% |
| xml | 1131 | 1117 | −1% |

Typical gap: **0–15% slower on compression, 0–27% slower on decompression**.
At `lz4hc -1` many files are within measurement noise (0–2%).

> Full per-file tables for all five codec variants (`lz4`, `lz4hc -1/-4/-8/-12`),
> correctness verification (byte-for-byte size identity across all 12 Silesia files),
> and lzbench integration instructions are in
> [docs/benchmark-results.md](docs/benchmark-results.md).

---

## Migration Methodology

This crate was ported from LZ4 v1.10.0 (≈13,000 lines across 11 C source / header
files) using **[AAMF](https://github.com/jafreck/AAMF)** (Automated Architecture
Migration Framework), an AI-assisted toolchain powered by Claude Sonnet 4.6.

### Strategy

Migration followed a **bottom-up, dependency-ordered** approach across 21 tasks in
6 serial phases so that each module could be built and tested in isolation before
any downstream consumer depended on it:

```
lz4.c (block) → xxhash (crate) → lz4hc.c → lz4frame.c → lz4file.c → lib.rs
```

### Source → target file map (abbreviated)

| C source | Rust target |
|----------|-------------|
| `lz4.c` / `lz4.h` | `src/block/{types,compress,stream,decompress_core,decompress_api}.rs` |
| `lz4hc.c` / `lz4hc.h` | `src/hc/{types,encode,lz4mid,search,compress_hc,dispatch,api}.rs` |
| `lz4frame.c` / headers | `src/frame/{types,header,cdict,compress,decompress}.rs` |
| `lz4file.c` / `lz4file.h` | `src/file.rs` |
| `xxhash.c` / `xxhash.h` | `xxhash-rust` crate (intentional substitution) |
| All public headers | `src/lib.rs` |

### Key decisions

- **`xxhash` replaced by crate** — `xxhash.c` (~1,000 lines of endianness / SIMD
  `#ifdef` chains) was substituted with `xxhash-rust = "0.8"`, verified to produce
  identical wire output.
- **Large C files split into focused modules** — `lz4.c` (2,829 lines) and
  `lz4hc.c` (2,192 lines) were decomposed into Rust modules of 150–400 lines each.
- **`unsafe` confined to hot paths** — pointer arithmetic in the decompressor core
  and C-ABI shims is kept in dedicated files; the rest of the API surface is safe.
- **`malloc`/`free` → RAII** — all state objects become `Box<T>` with `Drop` impls;
  no explicit memory management at call sites.

> See [docs/migration-summary.md](docs/migration-summary.md) and
> [docs/decision-log.md](docs/decision-log.md) for the complete record.

---

## Documentation

| Document | Description |
|----------|-------------|
| [docs/architecture-guide.md](docs/architecture-guide.md) | Module structure, layer diagram, and design rationale |
| [docs/api-reference.md](docs/api-reference.md) | Public API surface — functions, types, and error codes |
| [docs/developer-guide.md](docs/developer-guide.md) | Build, test, lint, and contribution workflow |
| [docs/benchmark-results.md](docs/benchmark-results.md) | Full lzbench Rust-vs-C throughput tables and methodology |
| [docs/migration-summary.md](docs/migration-summary.md) | Source-to-target file map and pattern mapping reference |
| [docs/decision-log.md](docs/decision-log.md) | Architectural decisions with rationale and rejected alternatives |
| [docs/known-issues.md](docs/known-issues.md) | Known warnings, limitations, and open items |

---

## License

GPL-2.0-only — same as the [upstream LZ4 programs](https://github.com/lz4/lz4).
The LZ4 library itself (which this crate ports) is BSD-2-Clause.
