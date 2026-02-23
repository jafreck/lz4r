# Migration Summary: lz4-to-rust

**Migration**: lz4-to-rust  
**Source**: LZ4 v1.10.0 (C)  
**Target**: Rust (stable, edition 2021)  
**Date**: 2026-02-22  
**Final Status**: ✅ PASS

---

## Overview

LZ4 v1.10.0 (11 C source/header files, ~13,000 lines) was migrated to a pure-Rust Cargo crate (`lz4 = "1.10.0"`) using a bottom-up, dependency-ordered strategy across 21 migration tasks and 6 serial phases. The migration took approximately 6 hours of automated execution.

---

## Source → Target File Mapping

| Source File(s) | Target File(s) | Task(s) | Status |
|----------------|---------------|---------|--------|
| `lz4.h`, `lz4hc.h`, `lz4frame.h`, `lz4frame_static.h`, `lz4file.h` | `Cargo.toml` | task-001 | ✅ |
| `xxhash.c`, `xxhash.h` | `src/xxhash.rs` (crate wrapper) | task-002 | ✅ (intentional crate substitution) |
| `lz4.c`/`lz4.h` (lines 239–740) | `src/block/types.rs` | task-003 | ✅ |
| `lz4.c`/`lz4.h` (lines 924–1524) | `src/block/compress.rs` | task-004 | ✅ |
| `lz4.c`/`lz4.h` (lines 1526–1834) | `src/block/stream.rs` | task-005 | ✅ |
| `lz4.c`/`lz4.h` (lines 1838–2447) | `src/block/decompress_core.rs` | task-006 | ✅ |
| `lz4.c`/`lz4.h` (lines 2448–2760) | `src/block/decompress_api.rs` | task-007 | ✅ |
| `lz4hc.c`/`lz4hc.h` (lines 71–260) | `src/hc/types.rs` | task-008 | ✅ |
| `lz4hc.c` (lines 262–355) | `src/hc/encode.rs` | task-009 | ✅ |
| `lz4hc.c` (lines 357–775) | `src/hc/lz4mid.rs` | task-010 | ✅ |
| `lz4hc.c` (lines 776–1000) | `src/hc/search.rs` | task-011 | ✅ |
| `lz4hc.c` (lines 1001–1485) | `src/hc/compress_hc.rs`, `src/hc/dispatch.rs` | task-012, task-013 | ✅ |
| `lz4hc.c` (lines 1486–2192) | `src/hc/api.rs` | task-014 | ✅ |
| `lz4frame.h`, `lz4frame_static.h` (types) | `src/frame/types.rs` | task-015 | ✅ |
| `lz4frame.c` (header encode/decode) | `src/frame/header.rs` | task-016 | ✅ |
| `lz4frame.c` (dict) | `src/frame/cdict.rs` | task-017 | ✅ |
| `lz4frame.c` (compress path) | `src/frame/compress.rs` | task-018 | ✅ |
| `lz4frame.c` (decompress path) | `src/frame/decompress.rs` | task-019 | ✅ |
| `lz4file.c`, `lz4file.h` | `src/file.rs` | task-020 | ✅ |
| All headers (public API) | `src/lib.rs` | task-021 | ✅ |

### Intentionally Omitted Files

| Source File | Reason |
|-------------|--------|
| `xxhash.c` / `xxhash.h` | Replaced by `xxhash-rust = "0.8"` crate (per migration plan) |
| `lib/Makefile` | Superseded by `Cargo.toml` |
| `lib/liblz4.pc.in` | pkg-config — not applicable to Rust |
| `lib/liblz4-dll.rc.in` | Windows DLL resource — not applicable |
| `lib/dll/` | Windows DLL example — not applicable |

---

## Source → Target Pattern Mappings

### Memory Management

| C Pattern | Rust Pattern | Notes |
|-----------|-------------|-------|
| `malloc` / `free` | `Box::new` / `Drop` | RAII; no explicit free needed |
| `calloc` | `Box::new(zeroed())` or `vec![0u8; n]` | |
| `memcpy` | `core::ptr::copy_nonoverlapping` (unsafe) or `slice::copy_from_slice` | |
| `memset` | `core::ptr::write_bytes` or `[0u8; N]` | |
| `memmove` | `core::ptr::copy` | |
| Stack allocation of state (`LZ4_HEAPMODE=0`) | `Box<T>` on heap | Simplifies lifetime management |
| Union for opaque sizing (`LZ4_stream_u`) | `Box<Lz4Stream>` / `Box<Lz4StreamHc>` | No stack-allocation shortcut needed |

### Error Handling

| C Pattern | Rust Pattern |
|-----------|-------------|
| Negative int return (`< 0 = error`) | `Err(Lz4Error::...)` variant |
| Zero return (`0 = failure`) | `Err(Lz4Error::OutputTooSmall)` |
| `LZ4F_errorCode_t` (size_t, negative range) | `Err(Lz4FError::...)` |
| `LZ4F_isError(code)` check | `Result` — errors are `Err(...)` |
| `XXH_errorcode` enum | `bool` / `Result` (wrapped by `xxhash-rust`) |

### Control Flow

| C Pattern | Rust Pattern |
|-----------|-------------|
| `goto _output_error` (error exit) | `return Err(...)` |
| `goto _last_literals` (loop exit to end) | `break 'compress` (labeled break on outer loop) |
| `goto _copy_continue` (overlap copy loop) | `loop { ... continue 'copy }` |
| `LZ4MID_compress` 8× goto | Mix of labeled breaks and `return Err(...)` |
| `LZ4HC_compress_optimal` loop-restart goto | `continue 'retry` |
| Error cleanup with resource free | `Drop` implementations + early `return Err(...)` |

### Platform Abstractions

| C Pattern | Rust Pattern |
|-----------|-------------|
| `LZ4_isLittleEndian()` union trick | `cfg!(target_endian = "little")` |
| `__builtin_ctzll(v)` | `v.trailing_zeros()` |
| `__builtin_clzll(v)` | `v.leading_zeros()` |
| `__builtin_bswap32(v)` | `v.swap_bytes()` |
| `likely(x)` / `unlikely(x)` branch hints | `std::hint::likely` / `std::hint::unlikely` (Rust 1.86+) or elided |
| `LZ4_FORCE_INLINE` | `#[inline(always)]` |
| `LZ4_FORCE_MEMORY_ACCESS` (3 modes) | `core::ptr::read_unaligned` + `core::ptr::write_unaligned` inside `unsafe` |
| `#ifdef __LP64__` / pointer-width guards | `cfg!(target_pointer_width = "64")` |

### Type System

| C Type | Rust Type |
|--------|----------|
| `LZ4_byte` / `BYTE` / `uint8_t` | `u8` |
| `LZ4_u16` / `U16` / `uint16_t` | `u16` |
| `LZ4_u32` / `U32` / `uint32_t` | `u32` |
| `LZ4_i8` / `int8_t` | `i8` |
| `reg_t` (pointer-sized uint) | `usize` |
| `size_t` | `usize` |
| `const char*` / `const LZ4_byte*` | `*const u8` (inside unsafe) or `&[u8]` |
| `char*` / `LZ4_byte*` (output) | `*mut u8` (inside unsafe) or `&mut [u8]` |
| Opaque `LZ4_stream_t` | `Lz4Stream` (struct, `Box<Lz4Stream>`) |
| Opaque `LZ4_streamHC_t` | `Lz4StreamHc` (struct, `Box<Lz4StreamHc>`) |
| Opaque `LZ4F_cctx*` | `Box<Lz4FCCtx>` |
| Opaque `LZ4F_dctx*` | `Box<Lz4FDCtx>` |
| `LZ4F_CDict*` | `Box<Lz4FCDict>` |
| `FILE*` | Generic `R: Read` / `W: Write` |
| `LZ4F_CustomMem` | Global allocator (feature gap; see Known Issues) |
| `XXH32_state_t` | `xxhash_rust::xxh32::Xxh32` (aliased as `Xxh32State`) |
| `compressFunc_t` (function pointer) | Rust enum + `match` dispatch |
| `lz4hc_strat_e` enum | `HcStrategy` enum |
| `tableType_t` enum | `TableType` enum |
| `dict_directive` enum | `DictDirective` enum (internal) |

### API Surface Changes

| C Function | Rust Function | Change |
|-----------|--------------|--------|
| `LZ4_compress_default(src, dst, srcSize, dstCapacity)` → `int` | `compress_default(src: &[u8], dst: &mut [u8]) -> Result<usize, Lz4Error>` | Slice bounds replace raw sizes; `Result` replaces sentinel |
| `LZ4_decompress_safe(src, dst, compressedSize, maxDecompressedSize)` → `int` | `decompress_safe(src: &[u8], dst: &mut [u8]) -> Result<usize, DecompressError>` | Same pattern |
| `LZ4F_compressFrame(dst, dstCap, src, srcSize, prefs)` → `size_t` | `lz4f_compress_frame(dst: &mut [u8], src: &[u8], prefs: Option<&Preferences>) -> Result<usize, Lz4FError>` | `Option` for nullable prefs |
| `LZ4_decompress_fast*` family | **Not migrated** | Deprecated and unsafe |
| `LZ4F_createCompressionContext_advanced(cmem)` | **Not migrated** | Requires custom allocator (see Known Issues) |
| `FILE*`-based `lz4file` API | Generic `R: Read` / `W: Write` | More idiomatic; broader compatibility |

---

## Behavioral Differences

All behavioral differences are intentional:

1. **Error return style**: All functions return `Result<T, E>` instead of sentinel integers. Callers must `?`-propagate or `match` on errors.

2. **No `LZ4_decompress_fast`**: The deprecated unsafe family (which omits compressed-size validation) was not migrated. Users must use `decompress_safe` variants.

3. **No custom allocator hooks**: `LZ4F_CustomMem` / `_advanced` context creation functions were not migrated. All contexts use the Rust global allocator.

4. **Stack vs. heap allocation**: The C library supports stack-allocated state via the union trick (`LZ4_stream_u`). The Rust port always heap-allocates streaming contexts via `Box`. One-shot functions still use stack-local state internally.

5. **I/O abstraction**: `lz4file.c` used `FILE*` handles passed in by the caller. The Rust port uses `R: Read` / `W: Write` generics, which compose with `File`, `BufReader`, `Cursor`, network sockets, etc.

6. **xxhash**: The vendored `xxhash.c`/`xxhash.h` is replaced by the `xxhash-rust` crate. The wire format is identical; the implementation is different.

7. **Build warnings**: 16 build warnings remain (all `unused_assignments`, `unused_labels`, `dead_code`) from literal C→Rust translation. These do not affect correctness.

---

## Migration Statistics

| Metric | Value |
|--------|-------|
| Source files | 11 |
| Target source files | 20 (src/) |
| Migration tasks | 21 |
| Integration tests | 856/856 passing |
| Test suites | 23 integration + 2 doc-tests |
| Compilation errors | 0 |
| Build warnings | 16 (low severity) |
| Stubs / TODOs | 0 |
| Lines of Rust (approx.) | ~8,000 |

---

## Build Results

| Check | Result |
|-------|--------|
| `cargo check` | ✅ Pass (0 errors) |
| `cargo build` | ✅ Pass (0 errors, 16 warnings) |
| `cargo test` | ✅ Pass (856/856) |
| Cross-module imports | ✅ All resolve |
| Version match | ✅ `1.10.0` |
| License match | ✅ `BSD-2-Clause` |

---

## See Also

- [Architecture Guide](./architecture-guide.md)
- [API Reference](./api-reference.md)
- [Decision Log](./decision-log.md)
- [Known Issues](./known-issues.md)
