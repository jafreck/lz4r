# lz4

Pure-Rust port of the [LZ4 compression library](https://github.com/lz4/lz4) (v1.10.0).

This crate provides a faithful translation of the full LZ4 C API surface — block compression, high-compression mode, streaming frame format, and file I/O — with no unsafe code and no C dependencies.

## Features

- **Block API** — one-shot `compress_default`, `compress_fast`, `decompress_safe`, and partial decompression
- **High-Compression (HC)** — `compress_hc` with configurable compression levels (1–12)
- **Frame API** — `Lz4F`-prefixed streaming compress/decompress with content checksums, dictionary support, and auto-flush
- **File I/O** — `Lz4ReadFile` / `Lz4WriteFile` wrappers for `std::io::{Read, Write}`

## Usage

```toml
[dependencies]
lz4 = "1.10.0"
```

```rust
use lz4::{compress_default, decompress_safe};

let input = b"hello world hello world hello world";
let bound = lz4::compress_bound(input.len() as i32) as usize;
let mut compressed = vec![0u8; bound];

let compressed_size = compress_default(input, &mut compressed).unwrap();
compressed.truncate(compressed_size as usize);

let mut output = vec![0u8; input.len()];
let result = decompress_safe(&compressed, &mut output).unwrap();
assert_eq!(&output[..result as usize], &input[..]);
```

## Migration

This crate was ported from C using [AAMF](https://github.com/jafreck/AAMF) (Automated Architecture Migration Framework). Migration artifacts — decision logs, architecture guides, and per-task parity tests — are preserved in the [`migration/`](migration/) directory.

## License

BSD-2-Clause — same as the [upstream LZ4 library](https://github.com/lz4/lz4).
