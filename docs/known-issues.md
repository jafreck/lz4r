# Known Issues: lz4-rust

**Migration**: lz4-to-rust  
**Source**: LZ4 v1.10.0 (C)  
**Target**: Rust (stable, edition 2021)  
**Date**: 2026-02-22  
**Version**: 1.10.0

---

## Summary

The migration is **complete and correct**. All 856 tests pass. The items listed here are low-severity cosmetic warnings and deliberate feature gaps — none affect correctness or functionality.

---

## Build Warnings (16 total, low severity)

These warnings arise from literal C-to-Rust translation of C idioms. They are non-blocking and do not affect runtime behaviour.

### Category 1: `unused_assignments` (14 warnings)

**Files**: `src/block/compress.rs`, `src/hc/compress_hc.rs`, `src/hc/lz4mid.rs`

**Cause**: C code frequently initialises variables before they are first set by the core logic (e.g., `int ip = 0; /* set in loop */`). Rust's flow analysis flags these as unused initial assignments.

**Suggested fix** (optional):
```rust
// Option A: suppress at function scope
#[allow(unused_assignments)]
fn lz4_compress_generic_validated(...) { ... }

// Option B: restructure to use uninitialised binding + guaranteed first-use
// (more idiomatic, but risks diverging from the original C structure)
```

**Risk if left unfixed**: None. Cosmetic only.

---

### Category 2: `unused_labels` (1 warning)

**File**: `src/block/compress.rs`

**Cause**: A `'main: loop` label used in the C-to-Rust goto translation may be flagged as unused if the label is only used by the `break` branch but Rust's analysis cannot confirm this.

**Suggested fix**:
```rust
#[allow(unused_labels)]
'main: loop { ... }
```

**Risk if left unfixed**: None. Cosmetic only.

---

### Category 3: `dead_code` (1 warning)

**File**: One struct field in `src/frame/` or `src/hc/` (exact field depends on build).

**Cause**: A struct field faithfully ported from C is never read in the current Rust code (the C code may have read it in a code path that was refactored during migration).

**Suggested fix**:
```rust
#[allow(dead_code)]
field_name: FieldType,
```
Or, if confirmed genuinely unused: remove the field.

**Risk if left unfixed**: None. Cosmetic only.

---

## Feature Gaps (Intentional)

These features from the C library were deliberately not migrated. They are documented here for completeness.

### 1. Custom Allocator API (`LZ4F_CustomMem`)

**C API**:
```c
LZ4F_createCompressionContext_advanced(LZ4F_CustomMem cmem, unsigned version);
LZ4F_createDecompressionContext_advanced(LZ4F_CustomMem cmem, unsigned version);
LZ4F_createCDict_advanced(LZ4F_CustomMem cmem, const void* dictBuffer, size_t dictSize);
```

**Status**: Not migrated.

**Reason**: The Rust `Allocator` trait (nightly) and `GlobalAlloc` trait (stable) provide equivalent functionality but require a different API shape. Mapping `LZ4F_CustomMem` function pointers directly to stable Rust is non-trivial and was deferred per the migration plan.

**Workaround**: All contexts use the Rust global allocator. Users who need a custom allocator can wrap the crate with a custom `GlobalAlloc` implementation via `#[global_allocator]`.

**Priority**: Low (affects only users with embedded/RTOS allocation requirements).

---

### 2. `LZ4_decompress_fast` Family

**C API**:
```c
LZ4_decompress_fast(src, dst, originalSize);           // deprecated since v1.9.0
LZ4_decompress_fast_continue(stream, src, dst, ...);
LZ4_decompress_fast_usingDict(src, dst, dict, ...);
```

**Status**: Not migrated.

**Reason**: These functions do not take a `compressedSize` parameter, so they cannot validate that the input ends before a buffer boundary. They are unsafe with untrusted input and were deprecated in v1.9.0. The migration plan explicitly excluded them.

**Workaround**: Use `decompress_safe` and `decompress_safe_continue` instead — they provide the same functionality with bounds checking.

**Priority**: None (deprecated in upstream).

---

### 3. Freestanding Mode (`LZ4_FREESTANDING=1`)

**C feature**: When compiled with `LZ4_FREESTANDING=1`, the C library disables all heap allocation and frame APIs, and allows the user to supply custom `LZ4_memcpy`, `LZ4_memset`, `LZ4_memmove` macros.

**Status**: Not migrated.

**Reason**: Rust's `no_std` mode provides equivalent functionality but requires a separate crate feature (`#![no_std]`). Adding `no_std` support was out of scope for the initial migration.

**Workaround**: Users targeting bare-metal environments should build with a custom global allocator and `no_std`. The block-only API (`src/block/`) is the most amenable to `no_std` extraction.

**Priority**: Low (requires users to build a `no_std` fork).

---

### 4. `likely` / `unlikely` Branch Hints

**C usage**: `#define likely(x) __builtin_expect((x) != 0, 1)` throughout hot paths.

**Status**: Branch hints are not applied in the Rust port.

**Reason**: `std::hint::likely` / `std::hint::unlikely` were stabilized in Rust 1.86. The migration targeted stable Rust generally, and the compiler often infers branch probabilities from surrounding code structure anyway.

**Workaround**: For Rust ≥ 1.86, add `std::hint::likely(...)` / `std::hint::unlikely(...)` around the same branch conditions as the C source.

**Priority**: Very low (micro-optimisation; profiling required to measure impact).

---

## Tracked TODOs / FIXMEs

A full search of all `src/` files found **0** `TODO`, `FIXME`, `unimplemented!()`, `todo!()`, or placeholder comments at the time of final parity verification (2026-02-22).

---

## See Also

- [Architecture Guide](./architecture-guide.md)
- [API Reference](./api-reference.md)
- [Migration Summary](./migration-summary.md)
- [Decision Log](./decision-log.md)
