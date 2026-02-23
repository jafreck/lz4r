# E2E Test Suite 06: Error Handling

## Overview

This test suite validates that the LZ4 Rust implementation correctly handles error conditions and edge cases without panicking or causing undefined behavior. All error paths return proper `Result` types with descriptive error variants.

## Test File

- **Path**: `e2e/suite_06_error_handling.rs`
- **Cargo Entry**: `[[test]] name = "e2e_error_handling"`
- **Tests**: 19 total

## Test Coverage

### 1. Block Decompression Errors (3 tests)

- **test_decompress_dst_too_small**: Verifies that `decompress_safe` returns `Err(MalformedInput)` when the destination buffer is too small for the decompressed data.
- **test_decompress_corrupt_data**: Tests that random garbage data returns an error instead of panicking.
- **test_decompress_empty_input**: Confirms empty input is handled as malformed data.

### 2. Block Compression Errors (3 tests)

- **test_compress_dst_empty**: Verifies that an empty destination buffer returns `Err(OutputTooSmall)`.
- **test_compress_dst_too_small**: Tests insufficient destination buffer handling.
- **test_compress_empty_input**: Ensures empty input compression doesn't panic.

### 3. Acceleration Parameter Edge Cases (2 tests)

- **test_compress_fast_zero_acceleration**: Tests that `compress_fast` with `acceleration=0` is handled gracefully (no panic). The implementation should clamp to minimum value.
- **test_compress_fast_negative_acceleration**: Verifies negative acceleration values don't cause panics.

### 4. Frame Decompression Errors (5 tests)

- **test_frame_decompress_truncated**: Tests behavior when a valid frame is truncated mid-stream.
- **test_frame_decompress_invalid_magic**: Verifies that frames with invalid magic numbers (0xFF...) return `Err(FrameTypeUnknown)`.
- **test_frame_decompress_all_zeros**: Tests that all-zero data is detected as invalid.
- **test_frame_decompress_partial_header**: Confirms that partial headers either request more data or return an error.

### 5. Constants Validation (2 tests)

- **test_max_input_size_constant**: Validates `LZ4_MAX_INPUT_SIZE == 0x7E000000` (2,113,929,216 bytes).
- **test_compress_input_too_large**: Tests that `compress_bound` returns 0 for inputs exceeding the maximum size.

### 6. Partial Decompression Edge Cases (3 tests)

- **test_decompress_partial_target_exceeds_dst**: Verifies safe behavior when `target_output_size > dst.len()`.
- **test_decompress_partial_zero_target**: Tests requesting zero bytes of output.
- **test_decompress_partial_target_larger_than_original**: Ensures requesting more bytes than available is handled safely.

### 7. Additional Edge Cases (3 tests)

- **test_roundtrip_single_byte**: Validates compress/decompress of a single byte.
- **test_compress_large_repeated_data**: Tests that highly compressible data (10KB of 'A's) compresses efficiently and roundtrips correctly.

## Running the Tests

```bash
cargo test --test e2e_error_handling
```

## Test Results

```
running 19 tests
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

## Key Behaviors Validated

1. **No Panics**: All error conditions return `Result::Err` instead of panicking.
2. **Proper Error Types**: Errors use appropriate variants (`OutputTooSmall`, `MalformedInput`, `FrameTypeUnknown`, etc.).
3. **Buffer Safety**: Operations never write beyond buffer boundaries.
4. **Graceful Degradation**: Invalid parameters (e.g., acceleration=0) are handled by clamping or returning errors.
5. **Parity with C Implementation**: Error behavior matches the original LZ4 C library's error handling patterns.

## Integration with LZ4 API

This test suite validates the Rust API exported from the top-level `lib.rs`:

- `lz4_compress_default` (alias for `compress_default`)
- `lz4_decompress_safe` (alias for `decompress_safe`)
- `compress_fast`
- `decompress_safe_partial`
- `lz4f_compress_frame`
- `lz4f_decompress`

All tests use the public API as it would be used by client code.
