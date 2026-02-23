# Decision Log: lz4-rust

**Migration**: lz4-to-rust  
**Source**: LZ4 v1.10.0 (C)  
**Target**: Rust (stable, edition 2021)  
**Date**: 2026-02-22  
**Version**: 1.10.0

This document records key architectural and implementation decisions made during the migration, including the rationale and rejected alternatives.

---

## D-001: Bottom-Up Migration Strategy

**Decision**: Migrate modules in strict dependency order: `lz4-block` → `xxhash` (crate) → `lz4-hc` → `lz4-frame` → `lz4-file` → `lib.rs`.

**Rationale**: The C source has a strict linear dependency graph. Attempting to migrate in any other order would require stub implementations of foundational modules, increasing the risk of parity failures. Bottom-up ensures each module can be built and tested independently before downstream consumers depend on it.

**Alternatives considered**:
- *Risk-First*: Tackle highest-complexity modules first regardless of order. Rejected because `lz4.c` is simultaneously the highest-risk module AND the foundational dependency — the two strategies converge.
- *Top-Down*: Start with `lz4file.c` and stub dependencies. Rejected because stubs would mask parity failures until the end.

**Adjudicator decision**: Bottom-Up is the only viable strategy given the strict dependency graph.

---

## D-002: Replace xxhash with `xxhash-rust` Crate

**Decision**: Do not manually translate `xxhash.c`/`xxhash.h` (~1,000 lines). Instead, add `xxhash-rust = { version = "0.8", features = ["xxh32"] }` as a dependency and wrap it in `src/xxhash.rs`.

**Rationale**:
- `xxhash.c` contains complex endianness/alignment logic with 3 compile-time memory-access modes and architecture-specific `#ifdef` chains. Manual translation is high-risk with low value.
- `xxhash-rust` is a well-maintained, published crate with identical wire semantics, verified against the reference implementation.
- Reduces migration scope by ~1,000 lines with zero semantic risk.
- Parity check: `xxh32_oneshot(b"", 0) == 0x02CC5D05` passes against the reference vector.

**Alternatives considered**:
- *Manual translation of xxhash.c*: Rejected due to high complexity (3 memory-access modes, endianness detection tricks, SIMD paths) and existence of a high-quality crate.

**Impact**: `xxhash.c` and `xxhash.h` do not appear in the target file list. This is documented as an intentional omission in the parity report.

---

## D-003: Decompose Large C Files into Multiple Rust Modules

**Decision**: Translate `lz4.c` (2,829 lines) and `lz4hc.c` (2,192 lines) into multiple focused Rust files rather than one large file per C source.

**Rationale**:
- Rust's module system encourages single-responsibility files.
- Large files are harder to review, test, and maintain.
- Independent migration tasks for each sub-module allow parallel parity verification.
- The decomposition reflects natural internal boundaries already present in the C code (compression vs. decompression, types vs. implementation, API vs. internals).

**Decomposition**:
- `lz4.c` → `block/types.rs`, `block/compress.rs`, `block/stream.rs`, `block/decompress_core.rs`, `block/decompress_api.rs`
- `lz4hc.c` → `hc/types.rs`, `hc/encode.rs`, `hc/lz4mid.rs`, `hc/search.rs`, `hc/compress_hc.rs`, `hc/dispatch.rs`, `hc/api.rs`
- `lz4frame.c` → `frame/types.rs`, `frame/header.rs`, `frame/cdict.rs`, `frame/compress.rs`, `frame/decompress.rs`

---

## D-004: RAII for Context Lifecycle (Replace Create/Free Pairs)

**Decision**: Implement `Drop` on all streaming context types to automatically free internal resources. Use `Box<T>` as the canonical heap-allocated handle.

**Rationale**: C's `LZ4_createStream`/`LZ4_freeStream` (and all equivalent pairs) are a classic RAII pattern that Rust can express idiomatically via `Drop`. This eliminates an entire category of resource leak bugs.

**Mapping**:
| C | Rust |
|---|------|
| `LZ4_createStream()` | `Lz4Stream::create()` → `Box<Lz4Stream>` |
| `LZ4_freeStream(stream)` | `drop(stream)` / automatic via `Box` |
| `LZ4_createStreamHC()` | `Lz4StreamHc::create()` → `Box<Lz4StreamHc>` |
| `LZ4F_createCompressionContext(...)` | `lz4f_create_compression_context(...)` → `Box<Lz4FCCtx>` |
| `LZ4F_createDecompressionContext(...)` | `lz4f_create_decompression_context(...)` → `Box<Lz4FDCtx>` |

---

## D-005: Result<T, E> Instead of Sentinel Return Values

**Decision**: All public functions return `Result<T, E>` where the C equivalent returns a signed integer or size_t with error sentinel values.

**Rationale**:
- Rust's type system makes it impossible to accidentally ignore errors when using `Result`.
- Callers can use `?` for clean error propagation.
- Error types (`Lz4Error`, `DecompressError`, `Lz4FError`) carry semantic information rather than opaque integers.

**Error type mapping**:
| C convention | Rust |
|-------------|------|
| Block compress: 0 = output too small, negative = other error, positive = success | `Err(Lz4Error::OutputTooSmall)` / `Ok(n)` |
| Block decompress: negative = error, positive = bytes written | `Err(DecompressError::MalformedInput)` / `Ok(n)` |
| Frame API: `LZ4F_errorCode_t` via `LZ4F_isError()` | `Err(Lz4FError::...)` / `Ok(n)` |

---

## D-006: goto Replacement Strategy

**Decision**: Map C `goto` statements to Rust control flow constructs on a case-by-case basis based on goto pattern type.

**Mapping used**:
| goto pattern | Rust equivalent |
|-------------|----------------|
| Error cleanup (`goto _output_error`) | `return Err(...)` |
| Hot-path loop exit (`goto _last_literals`) | `break 'compress` (labeled outer loop break) |
| Overlap-copy loop restart (`goto _copy_continue`) | `loop { ... continue 'copy }` |
| Loop retry (`LZ4HC_compress_optimal` retry) | `continue 'retry` |

**Total gotos converted**: 40 (13 in `lz4.c`, 24 in `lz4hc.c`, 3 in `lz4frame.c`).

**Rationale**: Each pattern type has a natural Rust idiom. No `unsafe` goto-equivalent (`setjmp`/`longjmp`) was needed.

---

## D-007: `unsafe` Confinement Policy

**Decision**: Confine `unsafe` code to the minimum necessary scope, always accompanied by a `// SAFETY:` comment explaining the invariant.

**Where `unsafe` is used**:
- `block/types.rs`: Raw pointer arithmetic in memory read/write helpers (`lz4_read32`, `lz4_write16`, `lz4_wild_copy8`, etc.)
- `block/compress.rs`: Hot-path hash table operations with raw pointers
- `block/decompress_core.rs`: All inner-loop pointer arithmetic (security-critical — all C bounds checks preserved)
- `block/stream.rs`: `attach_dictionary` (cross-stream raw pointer)
- `hc/` modules: Hash/chain table pointer arithmetic

**Where `unsafe` is NOT used**:
- `src/xxhash.rs` — zero unsafe (crate API is safe)
- `src/lib.rs` — zero unsafe (re-exports only)
- Public API wrappers — convert raw unsafe internals to safe `&[u8]` / `&mut [u8]` interfaces

---

## D-008: `FILE*` → Generic `R: Read` / `W: Write`

**Decision**: Replace `FILE*` parameters in `lz4file.c` with Rust generic type parameters `R: std::io::Read` and `W: std::io::Write`.

**Rationale**:
- More idiomatic Rust: works with `File`, `BufReader<File>`, `Cursor<Vec<u8>>`, network streams, etc.
- No loss of capability: `File` implements both traits.
- Avoids `unsafe` C FFI for file handles.
- I/O errors propagate as `std::io::Error` via the `Result` type.

**Trade-off**: The Rust API is generic; calling from C FFI requires concrete type instantiation. Not a concern for a pure-Rust library.

---

## D-009: Do Not Migrate Deprecated C Functions

**Decision**: The following deprecated C functions were intentionally not migrated:
- `LZ4_decompress_fast` / `LZ4_decompress_fast_continue` / `LZ4_decompress_fast_usingDict` (deprecated v1.9.0)
- `LZ4_compress_limitedOutput` (deprecated alias)
- Any function marked `LZ4_DEPRECATED` in `lz4.h`

**Rationale**: Deprecated functions are security risks (especially `LZ4_decompress_fast`, which has no compressed-size parameter) and are on a path to removal. Including them would perpetuate unsafe patterns in the Rust codebase.

---

## D-010: Stack vs. Heap Allocation (Always Box Streaming Contexts)

**Decision**: All streaming contexts are heap-allocated (`Box<T>`). The C stack-allocation shortcut via union sizing (`LZ4_stream_u` with `char minStateSize[]`) is not replicated.

**Rationale**:
- The union trick in C requires the internal struct layout to be stable and size-predictable. Rust structs are not guaranteed to have a stable ABI.
- Heap allocation via `Box` is idiomatic for large, long-lived state objects.
- `LZ4_stream_t` is 16 KB and `LZ4_streamHC_t` is ~256 KB — too large for the default stack in many contexts anyway.
- One-shot functions (e.g., `compress_default`) still use stack-local state internally; only the streaming API requires heap allocation.

---

## D-011: Module-Level Re-exports via `lib.rs`

**Decision**: All public API items are re-exported from `src/lib.rs` using `pub use` declarations, mirroring the C headers' role as the single point of API exposure.

**Rationale**: Users of the crate should be able to `use lz4::compress_default` rather than `use lz4::block::compress::compress_default`. This matches the flat API surface of the C headers.

---

## See Also

- [Architecture Guide](./architecture-guide.md)
- [API Reference](./api-reference.md)
- [Migration Summary](./migration-summary.md)
- [Known Issues](./known-issues.md)
