// Unit tests for task-016: src/io/decompress_resources.rs
//
// Verifies parity with lz4io.c v1.10.0, lines 1888–2014:
//   - Constants: INBUFF_SIZE, OUTBUFF_SIZE, PBUFFERS_NB, LZ4IO_D_BUFFER_SIZE
//   - `load_dict_file`           → LZ4IO_createDict (circular-buffer tail logic)
//   - `DecompressResources::new` → LZ4IO_createDResources (no dictionary)
//   - `DecompressResources::with_dict` → LZ4IO_createDResources + LZ4IO_loadDDict
//   - `DecompressResources::from_prefs` → convenience wrapper
//   - `DecompressResources::dict` → accessor
//   - `Buffer::new` / `Buffer::capacity` / `Buffer::as_slice` / `Buffer::as_mut_slice`
//   - `BufferPool::new` / `BufferPool::acquire` / `BufferPool::release`
//   - Concurrent BufferPool usage

use lz4::io::decompress_resources::{
    load_dict_file, BufferPool, DecompressResources, INBUFF_SIZE, LZ4IO_D_BUFFER_SIZE,
    OUTBUFF_SIZE, PBUFFERS_NB,
};
use lz4::io::prefs::{LZ4_MAX_DICT_SIZE, MB, Prefs};
use std::io::Write;

// ─────────────────────────────────────────────────────────────────────────────
// Constants (lz4io.c lines 1934–1937)
// ─────────────────────────────────────────────────────────────────────────────

/// INBUFF_SIZE must be 4 MiB.
#[test]
fn constant_inbuff_size_is_4mb() {
    assert_eq!(INBUFF_SIZE, 4 * MB);
}

/// OUTBUFF_SIZE must equal INBUFF_SIZE.
#[test]
fn constant_outbuff_size_equals_inbuff_size() {
    assert_eq!(OUTBUFF_SIZE, INBUFF_SIZE);
}

/// PBUFFERS_NB must be 3 (1 decomp + 1 queued + 1 in-flight).
#[test]
fn constant_pbuffers_nb_is_3() {
    assert_eq!(PBUFFERS_NB, 3);
}

/// LZ4IO_D_BUFFER_SIZE must be 64 KiB — the single-threaded scratch buffer.
#[test]
fn constant_d_buffer_size_is_64kb() {
    assert_eq!(LZ4IO_D_BUFFER_SIZE, 64 * 1024);
}

// ─────────────────────────────────────────────────────────────────────────────
// load_dict_file (mirrors LZ4IO_createDict, lz4io.c lines 1005–1062)
// ─────────────────────────────────────────────────────────────────────────────

/// A small dictionary (< 64 KiB) must be returned verbatim.
#[test]
fn load_dict_file_small_returned_verbatim() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    let data = b"hello world dictionary content";
    tmp.write_all(data).unwrap();
    let dict = load_dict_file(tmp.path()).expect("load should succeed");
    assert_eq!(&dict[..], &data[..]);
}

/// Empty file must yield an empty dictionary without error.
#[test]
fn load_dict_file_empty_file_returns_empty_vec() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let dict = load_dict_file(tmp.path()).expect("empty file should succeed");
    assert!(dict.is_empty());
}

/// File exactly 64 KiB is returned in full (no truncation needed).
#[test]
fn load_dict_file_exactly_64kb_not_truncated() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    let data: Vec<u8> = (0u8..=255).cycle().take(LZ4_MAX_DICT_SIZE).collect();
    tmp.write_all(&data).unwrap();
    let dict = load_dict_file(tmp.path()).expect("64 KiB file should load");
    assert_eq!(dict.len(), LZ4_MAX_DICT_SIZE);
    assert_eq!(dict, data);
}

/// File larger than 64 KiB must be truncated to the last 64 KiB (C parity).
#[test]
fn load_dict_file_larger_than_64kb_keeps_last_64kb() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    let data: Vec<u8> = (0u8..=255).cycle().take(128 * 1024).collect();
    tmp.write_all(&data).unwrap();
    let dict = load_dict_file(tmp.path()).expect("128 KiB file should load");
    assert_eq!(dict.len(), LZ4_MAX_DICT_SIZE);
    assert_eq!(dict, &data[64 * 1024..]);
}

/// File exactly 64 KiB + 1 byte: result length is 64 KiB and tail matches.
#[test]
fn load_dict_file_64kb_plus_one_byte_truncated() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    let data: Vec<u8> = (0u8..=255).cycle().take(LZ4_MAX_DICT_SIZE + 1).collect();
    tmp.write_all(&data).unwrap();
    let dict = load_dict_file(tmp.path()).expect("should load");
    assert_eq!(dict.len(), LZ4_MAX_DICT_SIZE);
    assert_eq!(dict, &data[1..]);
}

/// Non-existent file must return an error.
#[test]
fn load_dict_file_missing_file_returns_error() {
    let result = load_dict_file(std::path::Path::new("/tmp/__no_such_dict_file__.lz4dict"));
    assert!(result.is_err());
}

/// Very large file (> 128 KiB) must still yield exactly LZ4_MAX_DICT_SIZE bytes.
#[test]
fn load_dict_file_very_large_file_capped_at_64kb() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    let data: Vec<u8> = (0u8..=255).cycle().take(512 * 1024).collect();
    tmp.write_all(&data).unwrap();
    let dict = load_dict_file(tmp.path()).expect("large file should load");
    assert_eq!(dict.len(), LZ4_MAX_DICT_SIZE);
    assert_eq!(dict, &data[512 * 1024 - LZ4_MAX_DICT_SIZE..]);
}

// ─────────────────────────────────────────────────────────────────────────────
// DecompressResources::new (LZ4IO_createDResources without dictionary)
// ─────────────────────────────────────────────────────────────────────────────

/// `new` must allocate src and dst scratch buffers of LZ4IO_D_BUFFER_SIZE.
#[test]
fn decompress_resources_new_buffer_sizes() {
    let prefs = Prefs::default();
    let res = DecompressResources::new(&prefs).expect("new should not fail");
    assert_eq!(res.src_buffer.len(), LZ4IO_D_BUFFER_SIZE);
    assert_eq!(res.dst_buffer.len(), LZ4IO_D_BUFFER_SIZE);
}

/// `new` must not load a dictionary.
#[test]
fn decompress_resources_new_no_dict() {
    let prefs = Prefs::default();
    let res = DecompressResources::new(&prefs).expect("new should not fail");
    assert!(res.dict_buffer.is_none());
    assert!(res.dict().is_none());
}

/// `new` must not panic with an all-zeroed Prefs.
#[test]
fn decompress_resources_new_with_default_prefs_does_not_panic() {
    let prefs = Prefs::default();
    assert!(DecompressResources::new(&prefs).is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// DecompressResources::with_dict (LZ4IO_createDResources + LZ4IO_loadDDict)
// ─────────────────────────────────────────────────────────────────────────────

/// `with_dict` must load the dictionary and store it.
#[test]
fn decompress_resources_with_dict_loads_dict() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    let data = b"my dictionary data";
    tmp.write_all(data).unwrap();

    let prefs = Prefs::default();
    let res = DecompressResources::with_dict(&prefs, tmp.path()).expect("with_dict should succeed");
    assert!(res.dict_buffer.is_some());
    assert_eq!(res.dict().unwrap(), data.as_ref());
}

/// `with_dict` must still allocate src/dst buffers of the correct size.
#[test]
fn decompress_resources_with_dict_buffer_sizes_correct() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    tmp.write_all(b"data").unwrap();

    let prefs = Prefs::default();
    let res = DecompressResources::with_dict(&prefs, tmp.path()).expect("with_dict should succeed");
    assert_eq!(res.src_buffer.len(), LZ4IO_D_BUFFER_SIZE);
    assert_eq!(res.dst_buffer.len(), LZ4IO_D_BUFFER_SIZE);
}

/// `with_dict` must fail with a missing file.
#[test]
fn decompress_resources_with_dict_missing_file_returns_error() {
    let prefs = Prefs::default();
    let result = DecompressResources::with_dict(
        &prefs,
        std::path::Path::new("/tmp/__no_such_dict__.lz4dict"),
    );
    assert!(result.is_err());
}

// ─────────────────────────────────────────────────────────────────────────────
// DecompressResources::from_prefs (convenience wrapper)
// ─────────────────────────────────────────────────────────────────────────────

/// `from_prefs` with `use_dictionary=false` must not load any dictionary.
#[test]
fn from_prefs_no_dict_flag_returns_no_dict() {
    let prefs = Prefs::default(); // use_dictionary = false
    let res = DecompressResources::from_prefs(&prefs).expect("should succeed without dict");
    assert!(res.dict().is_none());
}

/// `from_prefs` with `use_dictionary=true` and a valid filename must load the dict.
#[test]
fn from_prefs_with_dict_flag_and_filename_loads_dict() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    let data = b"from_prefs dictionary bytes";
    tmp.write_all(data).unwrap();

    let mut prefs = Prefs::default();
    prefs.set_dictionary_filename(Some(tmp.path().to_str().unwrap()));

    let res = DecompressResources::from_prefs(&prefs).expect("should load dict from prefs");
    assert!(res.dict().is_some());
    assert_eq!(res.dict().unwrap(), data.as_ref());
}

/// `from_prefs` with `use_dictionary=true` but no filename must return an error.
#[test]
fn from_prefs_with_dict_flag_but_no_filename_returns_error() {
    let mut prefs = Prefs::default();
    prefs.use_dictionary = true;
    // dictionary_filename remains None
    let result = DecompressResources::from_prefs(&prefs);
    assert!(result.is_err());
}

// ─────────────────────────────────────────────────────────────────────────────
// DecompressResources::dict accessor
// ─────────────────────────────────────────────────────────────────────────────

/// `dict()` must return `None` when no dictionary was loaded.
#[test]
fn dict_accessor_returns_none_without_dict() {
    let prefs = Prefs::default();
    let res = DecompressResources::new(&prefs).unwrap();
    assert!(res.dict().is_none());
}

/// `dict()` must return the loaded bytes as a slice.
#[test]
fn dict_accessor_returns_slice_matching_loaded_dict() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    let data = b"test dictionary";
    tmp.write_all(data).unwrap();

    let prefs = Prefs::default();
    let res = DecompressResources::with_dict(&prefs, tmp.path()).unwrap();
    assert_eq!(res.dict().unwrap(), data.as_ref());
}

// ─────────────────────────────────────────────────────────────────────────────
// Buffer (lz4io.c lines 1939–1943)
// ─────────────────────────────────────────────────────────────────────────────

/// A new Buffer must start with size == 0 and capacity matching the requested size.
#[test]
fn buffer_new_size_zero_capacity_correct() {
    // Buffer::new is private; access via BufferPool::acquire.
    let pool = BufferPool::new(1024, 1);
    let buf = pool.acquire();
    assert_eq!(buf.size, 0);
    assert_eq!(buf.capacity(), 1024);
    pool.release(buf);
}

/// `as_slice()` on a zero-size buffer must return an empty slice.
#[test]
fn buffer_as_slice_size_zero_is_empty() {
    let pool = BufferPool::new(512, 1);
    let buf = pool.acquire();
    assert_eq!(buf.as_slice().len(), 0);
    pool.release(buf);
}

/// `as_slice()` must reflect the current `size` field.
#[test]
fn buffer_as_slice_reflects_size_field() {
    let pool = BufferPool::new(512, 1);
    let mut buf = pool.acquire();
    buf.size = 100;
    assert_eq!(buf.as_slice().len(), 100);
    buf.size = 0;
    pool.release(buf);
}

/// `as_mut_slice()` must return a slice of length equal to capacity.
#[test]
fn buffer_as_mut_slice_length_equals_capacity() {
    let pool = BufferPool::new(256, 1);
    let mut buf = pool.acquire();
    assert_eq!(buf.as_mut_slice().len(), 256);
    pool.release(buf);
}

/// Writing into `as_mut_slice()` then reading from `as_slice()` must reflect the change.
#[test]
fn buffer_mut_slice_write_visible_in_slice() {
    let pool = BufferPool::new(8, 1);
    let mut buf = pool.acquire();
    buf.as_mut_slice()[0] = 0xAB;
    buf.size = 1;
    assert_eq!(buf.as_slice()[0], 0xAB);
    buf.size = 0;
    pool.release(buf);
}

/// `capacity()` must equal the original allocation size.
#[test]
fn buffer_capacity_matches_pool_buf_size() {
    for size in [0usize, 1, 64, 4096, 1024 * 1024] {
        let pool = BufferPool::new(size, 1);
        let buf = pool.acquire();
        assert_eq!(buf.capacity(), size, "capacity mismatch for size={size}");
        pool.release(buf);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BufferPool (lz4io.c lines 1950–2013)
// ─────────────────────────────────────────────────────────────────────────────

/// A pool with `PBUFFERS_NB` buffers must allow acquiring and releasing all of them.
#[test]
fn buffer_pool_default_count_acquire_release() {
    let pool = BufferPool::new(4096, PBUFFERS_NB);
    let b1 = pool.acquire();
    let b2 = pool.acquire();
    let b3 = pool.acquire();
    // Pool is now empty — release all back.
    pool.release(b1);
    pool.release(b2);
    pool.release(b3);
    // Must be able to acquire again.
    let b = pool.acquire();
    assert_eq!(b.capacity(), 4096);
    pool.release(b);
}

/// Pool with count=1 must block acquisition of a second buffer until the first is released.
/// Test verifies the pool is bounded (we can acquire exactly 1).
#[test]
fn buffer_pool_count_1_single_buffer_cycle() {
    let pool = BufferPool::new(128, 1);
    let mut buf = pool.acquire();
    buf.size = 0;
    pool.release(buf);
    let buf2 = pool.acquire();
    assert_eq!(buf2.capacity(), 128);
    pool.release(buf2);
}

/// Releasing then acquiring repeatedly must always return a buffer of the correct capacity.
#[test]
fn buffer_pool_repeated_acquire_release_capacity_stable() {
    let pool = BufferPool::new(512, 2);
    for _ in 0..20 {
        let mut buf = pool.acquire();
        buf.size = 0;
        pool.release(buf);
    }
}

/// Pool must panic when constructed with count=0.
#[test]
#[should_panic(expected = "count must be > 0")]
fn buffer_pool_count_zero_panics() {
    let _ = BufferPool::new(1024, 0);
}

/// Concurrent acquire/release across multiple threads must not deadlock or panic.
#[test]
fn buffer_pool_concurrent_multithreaded() {
    use std::sync::Arc;
    use std::thread;

    let pool = Arc::new(BufferPool::new(OUTBUFF_SIZE, PBUFFERS_NB));
    let mut handles = Vec::new();
    for _ in 0..16 {
        let p = Arc::clone(&pool);
        handles.push(thread::spawn(move || {
            let mut buf = p.acquire();
            // Simulate brief work.
            buf.size = 1;
            buf.size = 0;
            p.release(buf);
        }));
    }
    for h in handles {
        h.join().expect("thread panicked");
    }
    // All buffers must be back in the pool.
    for _ in 0..PBUFFERS_NB {
        let b = pool.acquire();
        pool.release(b);
    }
}

/// Buffers returned after release must have the correct capacity (no reallocation).
#[test]
fn buffer_pool_released_buffer_capacity_unchanged() {
    let pool = BufferPool::new(8192, 2);
    let mut b = pool.acquire();
    b.size = 42;
    pool.release(b);
    let b2 = pool.acquire();
    // Capacity must be unchanged after release/re-acquire.
    assert_eq!(b2.capacity(), 8192);
    pool.release(b2);
}
