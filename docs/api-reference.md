# API Reference: lz4-rust

**Migration**: lz4-to-rust  
**Source**: LZ4 v1.10.0 (C)  
**Target**: Rust (stable, edition 2021)  
**Date**: 2026-02-22  
**Version**: 1.10.0

All public items are re-exported from the crate root (`src/lib.rs`). Import with `use lz4::*` or use fully-qualified paths.

---

## Version API

```rust
pub const LZ4_VERSION_MAJOR:   i32 = 1;
pub const LZ4_VERSION_MINOR:   i32 = 10;
pub const LZ4_VERSION_RELEASE: i32 = 0;
pub const LZ4_VERSION_NUMBER:  i32 = 11000;  // 1*10000 + 10*100 + 0
pub const LZ4_VERSION_STRING: &str = "1.10.0";

pub fn version_number() -> i32;
pub fn version_string() -> &'static str;
```

**C equivalents**: `LZ4_VERSION_NUMBER`, `LZ4_VERSION_STRING`, `LZ4_versionNumber()`, `LZ4_versionString()`

---

## Block Compression API

> Module: `lz4::block::compress` (re-exported at crate root)

### Constants

```rust
pub const LZ4_MAX_INPUT_SIZE: i32;          // 0x7E000000 (~2 GB)
pub const LZ4_ACCELERATION_DEFAULT: i32;    // 1
pub const LZ4_ACCELERATION_MAX: i32;        // 65537
pub const LZ4_DISTANCE_MAX: usize;          // 65535 (64 KB window)
pub const LZ4_COMPRESS_INPLACE_MARGIN: usize; // LZ4_DISTANCE_MAX + 32
```

### Error Type

```rust
#[derive(Debug)]
pub enum Lz4Error {
    OutputTooSmall,
    InputTooLarge,
    InvalidArgument,
}
impl std::fmt::Display for Lz4Error { ... }
impl std::error::Error for Lz4Error {}
```

**C equivalent**: negative return value from block compress functions.

### `compress_bound`

```rust
pub fn compress_bound(input_size: i32) -> i32
```

Returns the maximum output buffer size needed to hold the compressed form of `input_size` bytes. Returns 0 if `input_size` is out of range.

**C equivalent**: `LZ4_compressBound` / `LZ4_COMPRESSBOUND`

### `compress_default`

```rust
pub fn compress_default(src: &[u8], dst: &mut [u8]) -> Result<usize, Lz4Error>
```

One-shot block compression with default acceleration (1). Returns the number of bytes written to `dst`.

**C equivalent**: `LZ4_compress_default`

**Example**:
```rust
let src = b"hello world, hello world!";
let mut dst = vec![0u8; lz4::compress_bound(src.len() as i32) as usize];
let n = lz4::lz4_compress_default(src, &mut dst)?;
dst.truncate(n);
```

### `compress_fast`

```rust
pub fn compress_fast(
    src: &[u8],
    dst: &mut [u8],
    acceleration: i32,
) -> Result<usize, Lz4Error>
```

One-shot block compression with tunable acceleration. Higher values trade compression ratio for speed. Clamped to `[1, LZ4_ACCELERATION_MAX]`.

**C equivalent**: `LZ4_compress_fast`

### `compress_fast_ext_state`

```rust
pub fn compress_fast_ext_state(
    state: &mut block::types::StreamStateInternal,
    src: &[u8],
    dst: &mut [u8],
    acceleration: i32,
) -> Result<usize, Lz4Error>
```

Like `compress_fast` but uses a caller-supplied state buffer. Performs a full reset before use.

**C equivalent**: `LZ4_compress_fast_extState`

### `compress_fast_ext_state_fast_reset`

```rust
pub fn compress_fast_ext_state_fast_reset(
    state: &mut block::types::StreamStateInternal,
    src: &[u8],
    dst: &mut [u8],
    acceleration: i32,
) -> Result<usize, Lz4Error>
```

Like `compress_fast_ext_state` but skips full table reset (valid only if the stream has not crossed a 64 KB boundary since last reset).

**C equivalent**: `LZ4_compress_fast_extState_fastReset`

### `compress_dest_size`

```rust
pub fn compress_dest_size(
    src: &[u8],
    dst: &mut [u8],
    src_size_ptr: &mut i32,
) -> Result<usize, Lz4Error>
```

Fills `dst` completely with as much compressed data as possible; writes how many source bytes were consumed into `*src_size_ptr`.

**C equivalent**: `LZ4_compress_destSize`

### `compress_dest_size_ext_state`

```rust
pub fn compress_dest_size_ext_state(
    state: &mut block::types::StreamStateInternal,
    src: &[u8],
    dst: &mut [u8],
    src_size_ptr: &mut i32,
) -> Result<usize, Lz4Error>
```

**C equivalent**: `LZ4_compress_destSize_extState` (static/experimental)

### `size_of_state`

```rust
pub fn size_of_state() -> i32
```

Size in bytes of `StreamStateInternal`. Useful when allocating external state buffers.

**C equivalent**: `LZ4_sizeofState()`

### In-Place Buffer Helpers

```rust
pub const fn decompress_inplace_margin(compressed_size: usize) -> usize
pub const fn decompress_inplace_buffer_size(decompressed_size: usize) -> usize
pub const fn compress_inplace_buffer_size(max_compressed_size: usize) -> usize
```

**C equivalents**: `LZ4_DECOMPRESS_INPLACE_MARGIN`, `LZ4_DECOMPRESS_INPLACE_BUFFER_SIZE`, `LZ4_COMPRESS_INPLACE_BUFFER_SIZE`

---

## Block Decompression API

> Module: `lz4::block::decompress_api` (re-exported at crate root)

### Error Type

```rust
#[derive(Debug)]
pub enum BlockDecompressError {
    MalformedInput,
    OutputTooSmall,
    InvalidArgument,
}
// Type alias at crate root:
pub use block::decompress_api::BlockDecompressError as DecompressError;
```

**C equivalent**: negative return value from block decompress functions.

### `decompress_safe`

```rust
pub fn decompress_safe(
    src: &[u8],
    dst: &mut [u8],
) -> Result<usize, DecompressError>
```

Safely decompresses one LZ4 block. Validates all bounds; malformed input returns `Err` rather than panicking or causing UB.

**C equivalent**: `LZ4_decompress_safe`

### `decompress_safe_partial`

```rust
pub fn decompress_safe_partial(
    src: &[u8],
    dst: &mut [u8],
    target_output_size: usize,
) -> Result<usize, DecompressError>
```

Decompress until at least `target_output_size` bytes are produced or input is exhausted.

**C equivalent**: `LZ4_decompress_safe_partial`

### `decompress_safe_using_dict`

```rust
pub fn decompress_safe_using_dict(
    src: &[u8],
    dst: &mut [u8],
    dict: &[u8],
) -> Result<usize, DecompressError>
```

**C equivalent**: `LZ4_decompress_safe_usingDict`

### `decompress_safe_partial_using_dict`

```rust
pub fn decompress_safe_partial_using_dict(
    src: &[u8],
    dst: &mut [u8],
    target_output_size: usize,
    dict: &[u8],
) -> Result<usize, DecompressError>
```

**C equivalent**: `LZ4_decompress_safe_partial_usingDict`

### `decoder_ring_buffer_size`

```rust
pub fn decoder_ring_buffer_size(max_block_size: i32) -> i32
```

Returns the minimum ring-buffer size (in bytes) for streaming decompression with the given maximum block size.

**C equivalent**: `LZ4_decoderRingBufferSize`

---

## Streaming Block Compression API

> Module: `lz4::block::stream` (re-exported at crate root)

### `Lz4Stream`

```rust
pub struct Lz4Stream { /* opaque */ }

impl Lz4Stream {
    /// Allocate and initialize a new streaming compression context.
    /// C equivalent: LZ4_createStream()
    pub fn create() -> Box<Lz4Stream>;

    /// Reset stream to initial state.
    /// C equivalent: LZ4_resetStream_fast()
    pub fn reset(&mut self);

    /// Load a dictionary into the stream for subsequent compression.
    /// C equivalent: LZ4_loadDict()
    pub fn load_dict(&mut self, dict: &[u8]) -> i32;

    /// Attach an external dictionary stream (must outlive `self`).
    /// # Safety: dict_stream must remain valid and unmodified for the
    /// lifetime of the attached compression session.
    /// C equivalent: LZ4_attach_dictionary()
    pub unsafe fn attach_dictionary(&mut self, dict_stream: Option<*const Lz4Stream>);

    /// Compress the next block in a streaming session.
    /// C equivalent: LZ4_compress_fast_continue()
    pub fn compress_fast_continue(
        &mut self,
        src: &[u8],
        dst: &mut [u8],
        acceleration: i32,
    ) -> Result<usize, Lz4Error>;

    /// Save up to the last 64 KB of history into dict.
    /// C equivalent: LZ4_saveDict()
    pub fn save_dict(&mut self, dict: &mut [u8]) -> i32;
}

impl Drop for Lz4Stream { /* frees internal state (C: LZ4_freeStream) */ }
```

**Example**:
```rust
let mut stream = lz4::Lz4Stream::create();
let mut out1 = vec![0u8; lz4::compress_bound(block1.len() as i32) as usize];
let n1 = stream.compress_fast_continue(block1, &mut out1, 1)?;
let mut out2 = vec![0u8; lz4::compress_bound(block2.len() as i32) as usize];
let n2 = stream.compress_fast_continue(block2, &mut out2, 1)?;
```

---

## Streaming Block Decompression API

> Module: `lz4::block::decompress_api` (re-exported at crate root)

### `Lz4StreamDecode`

```rust
pub struct Lz4StreamDecode { /* opaque */ }

impl Lz4StreamDecode {
    pub fn new() -> Self;
}

impl Drop for Lz4StreamDecode { /* C: LZ4_freeStreamDecode */ }
```

### `set_stream_decode`

```rust
pub fn set_stream_decode(
    stream: &mut Lz4StreamDecode,
    dict: &[u8],
) -> Result<(), DecompressError>
```

Initialize or reset a streaming decode context, optionally with a dictionary.

**C equivalent**: `LZ4_setStreamDecode`

### `decompress_safe_continue`

```rust
pub fn decompress_safe_continue(
    stream: &mut Lz4StreamDecode,
    src: &[u8],
    dst: &mut [u8],
) -> Result<usize, DecompressError>
```

**C equivalent**: `LZ4_decompress_safe_continue`

---

## High-Compression (HC) Block API

> Module: `lz4::hc::api` (re-exported at crate root via `lz4::hc`)

### Constants

```rust
pub const LZ4HC_CLEVEL_MIN:     i32 = 2;
pub const LZ4HC_CLEVEL_DEFAULT: i32 = 9;
pub const LZ4HC_CLEVEL_OPT_MIN: i32 = 10;
pub const LZ4HC_CLEVEL_MAX:     i32 = 12;
```

**C equivalents**: `LZ4HC_CLEVEL_MIN`, `LZ4HC_CLEVEL_DEFAULT`, `LZ4HC_CLEVEL_OPT_MIN`, `LZ4HC_CLEVEL_MAX`

### `sizeof_state_hc`

```rust
pub fn sizeof_state_hc() -> i32
```

**C equivalent**: `LZ4_sizeofStateHC()`

### `compress_hc`

```rust
pub fn compress_hc(
    src: &[u8],
    dst: &mut [u8],
    compression_level: i32,
) -> Result<usize, Lz4Error>
```

One-shot HC compression at the specified level (clamped to `[LZ4HC_CLEVEL_MIN, LZ4HC_CLEVEL_MAX]`).

**C equivalent**: `LZ4_compress_HC`

**Example**:
```rust
let mut dst = vec![0u8; lz4::compress_bound(src.len() as i32) as usize];
let n = lz4::hc::compress_hc(src, &mut dst, 9)?;
```

### `compress_hc_ext_state`

```rust
pub fn compress_hc_ext_state(
    state: &mut Lz4StreamHc,
    src: &[u8],
    dst: &mut [u8],
    compression_level: i32,
) -> Result<usize, Lz4Error>
```

**C equivalent**: `LZ4_compress_HC_extStateHC`

### `compress_hc_dest_size`

```rust
pub fn compress_hc_dest_size(
    state: &mut Lz4StreamHc,
    src: &[u8],
    dst: &mut [u8],
    src_size_ptr: &mut i32,
    compression_level: i32,
) -> Result<usize, Lz4Error>
```

**C equivalent**: `LZ4_compress_HC_destSize`

### `Lz4StreamHc`

```rust
pub struct Lz4StreamHc { /* opaque */ }

impl Lz4StreamHc {
    /// C equivalent: LZ4_createStreamHC()
    pub fn create() -> Box<Lz4StreamHc>;
}

impl Drop for Lz4StreamHc { /* C: LZ4_freeStreamHC */ }
```

### HC Stream Functions

```rust
pub fn init_stream_hc(stream: &mut Lz4StreamHc, compression_level: i32);
pub fn reset_stream_hc(stream: &mut Lz4StreamHc, compression_level: i32);
pub fn reset_stream_hc_fast(stream: &mut Lz4StreamHc, compression_level: i32);
pub fn set_compression_level(stream: &mut Lz4StreamHc, compression_level: i32);
pub fn favor_decompression_speed(stream: &mut Lz4StreamHc, favor: i32);
pub fn load_dict_hc(stream: &mut Lz4StreamHc, dict: &[u8]) -> i32;
pub fn save_dict_hc(stream: &mut Lz4StreamHc, dst: &mut [u8]) -> i32;
pub fn compress_hc_continue(
    stream: &mut Lz4StreamHc,
    src: &[u8],
    dst: &mut [u8],
) -> Result<usize, Lz4Error>;
```

**C equivalents**: `LZ4_initStreamHC`, `LZ4_resetStreamHC`, `LZ4_resetStreamHC_fast`, `LZ4_setCompressionLevel`, `LZ4_favorDecompressionSpeed`, `LZ4_loadDictHC`, `LZ4_saveDictHC`, `LZ4_compress_HC_continue`

---

## Frame Compression API

> Module: `lz4::frame::compress` (re-exported at `lz4::frame`)

### Frame Types

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlockSizeId { Default = 0, Max64Kb = 4, Max256Kb = 5, Max1Mb = 6, Max4Mb = 7 }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlockMode { Linked = 0, Independent = 1 }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContentChecksum { Disabled = 0, Enabled = 1 }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlockChecksum { Disabled = 0, Enabled = 1 }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrameType { Frame = 0, SkippableFrame = 1 }

#[derive(Debug, Clone, Default)]
pub struct FrameInfo {
    pub block_size_id: BlockSizeId,
    pub block_mode: BlockMode,
    pub content_checksum: ContentChecksum,
    pub frame_type: FrameType,
    pub content_size: u64,   // 0 = unknown
    pub dict_id: u32,         // 0 = no dict
    pub block_checksum: BlockChecksum,
}

#[derive(Debug, Clone, Default)]
pub struct Preferences {
    pub frame_info: FrameInfo,
    pub compression_level: i32,
    pub auto_flush: u32,
    pub favor_dec_speed: u32,
}

#[derive(Debug, Clone, Default)]
pub struct CompressOptions {
    pub stable_src: u32,
}
```

### `Lz4FError`

```rust
#[derive(Debug)]
pub enum Lz4FError {
    Generic,
    MaxBlockSizeInvalid,
    BlockModeInvalid,
    ParameterInvalid,
    CompressionLevelInvalid,
    HeaderVersionWrong,
    BlockChecksumInvalid,
    AllocationFailed,
    SrcSizeTooLarge,
    DstMaxSizeTooSmall,
    FrameHeaderIncomplete,
    FrameTypeUnknown,
    FrameSizeWrong,
    SrcPtrWrong,
    DecompressionFailed,
    HeaderChecksumInvalid,
    ContentChecksumInvalid,
    FrameDecodingAlreadyStarted,
    CompressionStateUninitialized,
    ParameterNull,
    IoWrite(std::io::Error),
    IoRead(std::io::Error),
}
impl std::fmt::Display for Lz4FError { ... }
impl std::error::Error for Lz4FError {}
```

**C equivalent**: `LZ4F_errorCodes` enum + `LZ4F_isError()` / `LZ4F_getErrorName()`

### `lz4f_compress_frame`

```rust
pub fn lz4f_compress_frame(
    dst: &mut [u8],
    src: &[u8],
    prefs: Option<&Preferences>,
) -> Result<usize, Lz4FError>
```

One-shot frame compression. Returns the number of bytes written to `dst`.

**C equivalent**: `LZ4F_compressFrame`

**Example**:
```rust
let mut dst = vec![0u8; lz4f_compress_bound(src.len(), None)];
let n = lz4f_compress_frame(&mut dst, src, None)?;
dst.truncate(n);
```

### `lz4f_compress_bound`

```rust
pub fn lz4f_compress_bound(src_size: usize, prefs: Option<&Preferences>) -> usize
```

Returns the maximum compressed output size for `src_size` bytes with the given preferences.

**C equivalent**: `LZ4F_compressBound`

### Streaming Frame Compression Lifecycle

```rust
/// Create a new compression context. Must call lz4f_free_compression_context when done.
/// C equivalent: LZ4F_createCompressionContext
pub fn lz4f_create_compression_context(version: u32) -> Result<Box<Lz4FCCtx>, Lz4FError>;

/// Write frame header to dst. Must be called once before any update.
/// C equivalent: LZ4F_compressBegin
pub fn lz4f_compress_begin(
    ctx: &mut Lz4FCCtx,
    dst: &mut [u8],
    prefs: Option<&Preferences>,
) -> Result<usize, Lz4FError>;

/// Compress a chunk of data. May be called repeatedly.
/// C equivalent: LZ4F_compressUpdate
pub fn lz4f_compress_update(
    ctx: &mut Lz4FCCtx,
    dst: &mut [u8],
    src: &[u8],
    opts: Option<&CompressOptions>,
) -> Result<usize, Lz4FError>;

/// Flush buffered data to dst (optional, for intermediate output).
/// C equivalent: LZ4F_flush
pub fn lz4f_flush(
    ctx: &mut Lz4FCCtx,
    dst: &mut [u8],
    opts: Option<&CompressOptions>,
) -> Result<usize, Lz4FError>;

/// Write end mark and optional content checksum. Must be called to finalize frame.
/// C equivalent: LZ4F_compressEnd
pub fn lz4f_compress_end(
    ctx: &mut Lz4FCCtx,
    dst: &mut [u8],
    opts: Option<&CompressOptions>,
) -> Result<usize, Lz4FError>;

/// Free the compression context.
/// C equivalent: LZ4F_freeCompressionContext
pub fn lz4f_free_compression_context(ctx: Box<Lz4FCCtx>);
```

### `lz4f_uncompressed_update`

```rust
pub fn lz4f_uncompressed_update(
    ctx: &mut Lz4FCCtx,
    dst: &mut [u8],
    src: &[u8],
    opts: Option<&CompressOptions>,
) -> Result<usize, Lz4FError>
```

Store `src` as an uncompressed block in the frame (static/advanced API).

**C equivalent**: `LZ4F_uncompressedUpdate`

---

## Frame Decompression API

> Module: `lz4::frame::decompress` (re-exported at `lz4::frame`)

### `DecompressOptions`

```rust
#[derive(Debug, Clone, Default)]
pub struct DecompressOptions {
    pub stable_dst: u32,
    pub skip_checksums: u32,
}
```

### `Lz4FDCtx`

```rust
pub struct Lz4FDCtx { /* opaque decompression context */ }
```

### Streaming Frame Decompression Lifecycle

```rust
/// C equivalent: LZ4F_createDecompressionContext
pub fn lz4f_create_decompression_context(version: u32) -> Result<Box<Lz4FDCtx>, Lz4FError>;

/// C equivalent: LZ4F_freeDecompressionContext
pub fn lz4f_free_decompression_context(ctx: Box<Lz4FDCtx>) -> usize;

/// Decompress one or more chunks. Returns (src_consumed, dst_written).
/// C equivalent: LZ4F_decompress
pub fn lz4f_decompress(
    ctx: &mut Lz4FDCtx,
    dst: &mut [u8],
    src: &[u8],
    opts: Option<&DecompressOptions>,
) -> Result<(usize, usize), Lz4FError>;

/// Read and return the frame info from the header.
/// C equivalent: LZ4F_getFrameInfo
pub fn lz4f_get_frame_info(
    ctx: &mut Lz4FDCtx,
    src: &[u8],
) -> Result<(FrameInfo, usize), Lz4FError>;

/// Reset the decompression context to accept a new frame.
/// C equivalent: LZ4F_resetDecompressionContext
pub fn lz4f_reset_decompression_context(ctx: &mut Lz4FDCtx);

/// Estimate the minimum header size needed to determine frame type.
/// C equivalent: LZ4F_headerSize
pub fn lz4f_header_size(src: &[u8]) -> Result<usize, Lz4FError>;

/// Decompress using an external dictionary.
/// C equivalent: LZ4F_decompress_usingDict
pub fn lz4f_decompress_using_dict(
    ctx: &mut Lz4FDCtx,
    dst: &mut [u8],
    src: &[u8],
    dict: &[u8],
    opts: Option<&DecompressOptions>,
) -> Result<(usize, usize), Lz4FError>;
```

### `lz4f_decompress` (top-level re-export)

```rust
/// C equivalent: LZ4F_decompress (one call per chunk)
pub use frame::decompress::lz4f_decompress;
```

---

## Frame Dictionary API

> Module: `lz4::frame::cdict`

### `Lz4FCDict`

```rust
pub struct Lz4FCDict { /* pre-loaded compression dictionary */ }

impl Lz4FCDict {
    /// Create a CDict from raw dictionary bytes.
    /// C equivalent: LZ4F_createCDict
    pub fn create(dict: &[u8]) -> Result<Box<Lz4FCDict>, Lz4FError>;
}

impl Drop for Lz4FCDict { /* C: LZ4F_freeCDict */ }
```

### `lz4f_compress_frame_using_cdict`

```rust
pub fn lz4f_compress_frame_using_cdict(
    dst: &mut [u8],
    src: &[u8],
    cdict: &Lz4FCDict,
    prefs: Option<&Preferences>,
) -> Result<usize, Lz4FError>
```

**C equivalent**: `LZ4F_compressFrame_usingCDict`

---

## File / Stream I/O API

> Module: `lz4::file`

### `Lz4ReadFile<R: Read>`

```rust
pub struct Lz4ReadFile<R: Read> { /* opaque */ }

impl<R: Read> Lz4ReadFile<R> {
    /// Open an LZ4 frame reader around `reader`.
    /// C equivalent: LZ4F_readOpen
    pub fn open(reader: R) -> Result<Self, Lz4FError>;
}

impl<R: Read> std::io::Read for Lz4ReadFile<R> {
    /// Decompress and read decompressed bytes into buf.
    /// C equivalent: LZ4F_read
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
}

impl<R: Read> Drop for Lz4ReadFile<R> { /* C: LZ4F_readClose */ }
```

### `Lz4WriteFile<W: Write>`

```rust
pub struct Lz4WriteFile<W: Write> { /* opaque */ }

impl<W: Write> Lz4WriteFile<W> {
    /// Open an LZ4 frame writer around `writer`.
    /// C equivalent: LZ4F_writeOpen
    pub fn open(writer: W, prefs: Option<&Preferences>) -> Result<Self, Lz4FError>;

    /// Finalize the frame and flush all data.
    /// C equivalent: LZ4F_writeClose (also called by Drop)
    pub fn close(self) -> Result<W, Lz4FError>;
}

impl<W: Write> std::io::Write for Lz4WriteFile<W> {
    /// Compress and write data.
    /// C equivalent: LZ4F_write
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize>;
    fn flush(&mut self) -> std::io::Result<()>;
}

impl<W: Write> Drop for Lz4WriteFile<W> { /* calls close if not already done */ }
```

### Convenience Functions

```rust
/// Decompress a complete LZ4 frame from reader into a Vec<u8>.
pub fn lz4_read_frame<R: Read>(reader: R) -> Result<Vec<u8>, Lz4FError>;

/// Compress data as a single LZ4 frame to writer.
pub fn lz4_write_frame<W: Write>(
    writer: W,
    data: &[u8],
    prefs: Option<&Preferences>,
) -> Result<usize, Lz4FError>;
```

**C equivalents**: `LZ4F_readOpen`, `LZ4F_read`, `LZ4F_readClose`, `LZ4F_writeOpen`, `LZ4F_write`, `LZ4F_writeClose`

---

## xxHash Utilities

> Module: `lz4::xxhash` (internal; not intended for direct use)

```rust
/// Streaming XXH32 state — re-exported from xxhash-rust.
/// C equivalent: XXH32_state_t
pub use xxhash_rust::xxh32::Xxh32 as Xxh32State;

/// One-shot XXH32 hash.
/// C equivalent: XXH32(data, len, seed)
pub fn xxh32_oneshot(data: &[u8], seed: u32) -> u32;
```

---

## Deprecated / Not Migrated

The following C functions are **not** present in the Rust crate, by design:

| C Function | Reason Omitted |
|-----------|---------------|
| `LZ4_decompress_fast` family | Deprecated since v1.9.0; unsafe (no compressed-size validation) |
| `LZ4_compress_limitedOutput` | Deprecated; replaced by `compress_fast` with bounded dst |
| `LZ4F_createCompressionContext_advanced` | Uses `LZ4F_CustomMem`; not applicable to stable Rust global allocator |
| `LZ4F_createDecompressionContext_advanced` | Same reason |
| `LZ4F_createCDict_advanced` | Same reason |
| `XXH32` / `XXH64` direct (from `xxhash.c`) | Replaced by `xxhash-rust` crate |

---

## See Also

- [Architecture Guide](./architecture-guide.md)
- [Developer Guide](./developer-guide.md)
- [Migration Summary](./migration-summary.md)
