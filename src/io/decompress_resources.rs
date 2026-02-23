//! Decompression context and buffer pool.
//!
//! This module provides two layers of resource management for the LZ4
//! decompression pipeline:
//!
//! - [`DecompressResources`]: scratch I/O buffers and an optional pre-loaded
//!   dictionary for the single-threaded decompression path.
//! - [`BufferPool`]: a fixed pool of reusable [`Buffer`] objects used by the
//!   multi-threaded decompression path.  Buffers are exchanged over a bounded
//!   channel, so acquisition blocks rather than busy-waits when all buffers
//!   are in use.
//!
//! Dictionary loading ([`load_dict_file`]) retains only the last 64 KiB of
//! the dictionary file, matching the LZ4 specification's maximum dictionary
//! size limit.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use crossbeam_channel::{bounded, Receiver, Sender};

use crate::io::prefs::{Prefs, LZ4_MAX_DICT_SIZE, MB};

// ---------------------------------------------------------------------------
// Buffer-size constants
// ---------------------------------------------------------------------------

/// Size of each MT decompression input buffer (4 MiB).
pub const INBUFF_SIZE: usize = 4 * MB;

/// Size of each MT decompression output buffer (same as input).
pub const OUTBUFF_SIZE: usize = INBUFF_SIZE;

/// Number of pre-allocated buffers in the pool:
/// 1 being decompressed + 1 in the output queue + 1 being written to I/O.
pub const PBUFFERS_NB: usize = 3;

/// Buffer size for the single-threaded decompression path (64 KiB).
pub const LZ4IO_D_BUFFER_SIZE: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// Dictionary loading
// ---------------------------------------------------------------------------

/// Loads the last `LZ4_MAX_DICT_SIZE` (64 KiB) bytes of `dict_path` into a
/// contiguous `Vec<u8>` using an in-place circular buffer.
///
/// If the file is shorter than 64 KiB the entire file is returned.  If it is
/// longer, only the trailing 64 KiB is retained — the portion that LZ4
/// decoders are permitted to reference as dictionary context.
///
/// Returns `io::Error` on any I/O failure.
pub fn load_dict_file(dict_path: &Path) -> io::Result<Vec<u8>> {
    let mut file = fs::File::open(dict_path)?;

    // Opportunistically seek to the tail of the file, ignoring errors
    // (the file might be stdin-like, in which case we just read it all).
    let _ = file.seek(SeekFrom::End(-(LZ4_MAX_DICT_SIZE as i64)));

    // Accumulate bytes into a circular buffer of capacity LZ4_MAX_DICT_SIZE.
    let mut circular: Vec<u8> = vec![0u8; LZ4_MAX_DICT_SIZE];
    let mut dict_end: usize = 0;
    let mut dict_len: usize = 0;

    loop {
        let cap = LZ4_MAX_DICT_SIZE - dict_end;
        let n = file.read(&mut circular[dict_end..dict_end + cap])?;
        if n == 0 {
            break;
        }
        dict_end = (dict_end + n) % LZ4_MAX_DICT_SIZE;
        dict_len += n;
    }

    if dict_len > LZ4_MAX_DICT_SIZE {
        dict_len = LZ4_MAX_DICT_SIZE;
    }

    // Linearise the circular buffer into a contiguous Vec<u8>.
    let dict_start = (LZ4_MAX_DICT_SIZE + dict_end - dict_len) % LZ4_MAX_DICT_SIZE;

    let mut out = Vec::with_capacity(dict_len);
    if dict_start + dict_len <= LZ4_MAX_DICT_SIZE {
        out.extend_from_slice(&circular[dict_start..dict_start + dict_len]);
    } else {
        // Wrap: tail portion then head portion.
        out.extend_from_slice(&circular[dict_start..]);
        out.extend_from_slice(&circular[..dict_end]);
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// DecompressResources
// ---------------------------------------------------------------------------

/// Decompression context and associated buffers.
///
/// Holds the scratch I/O buffers and optional pre-loaded dictionary needed by
/// the single-threaded decompression path.  The LZ4F decompression context is
/// created per-call inside the frame-decompression module rather than stored
/// here, keeping this struct FFI-free.
pub struct DecompressResources {
    /// Scratch buffer for reading compressed input (64 KiB).
    pub src_buffer: Vec<u8>,

    /// Scratch buffer for writing decompressed output (64 KiB).
    pub dst_buffer: Vec<u8>,

    /// Pre-loaded dictionary bytes, if any.
    pub dict_buffer: Option<Vec<u8>>,
}

impl DecompressResources {
    /// Creates decompression resources with no dictionary.
    pub fn new(_prefs: &Prefs) -> io::Result<Self> {
        Ok(DecompressResources {
            src_buffer: vec![0u8; LZ4IO_D_BUFFER_SIZE],
            dst_buffer: vec![0u8; LZ4IO_D_BUFFER_SIZE],
            dict_buffer: None,
        })
    }

    /// Creates decompression resources and loads the dictionary at `dict_path`.
    ///
    /// Only the last 64 KiB of the dictionary file is retained; see
    /// [`load_dict_file`] for details.
    pub fn with_dict(_prefs: &Prefs, dict_path: &Path) -> io::Result<Self> {
        let dict = load_dict_file(dict_path)?;
        Ok(DecompressResources {
            src_buffer: vec![0u8; LZ4IO_D_BUFFER_SIZE],
            dst_buffer: vec![0u8; LZ4IO_D_BUFFER_SIZE],
            dict_buffer: Some(dict),
        })
    }

    /// Creates decompression resources, loading a dictionary from
    /// `prefs.dictionary_filename` if `prefs.use_dictionary` is set.
    pub fn from_prefs(prefs: &Prefs) -> io::Result<Self> {
        if prefs.use_dictionary {
            let path = prefs.dictionary_filename.as_deref().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Dictionary error: no filename provided",
                )
            })?;
            Self::with_dict(prefs, Path::new(path))
        } else {
            Self::new(prefs)
        }
    }

    /// Returns the dictionary bytes if a dictionary was loaded.
    #[inline]
    pub fn dict(&self) -> Option<&[u8]> {
        self.dict_buffer.as_deref()
    }
}

// ---------------------------------------------------------------------------
// Buffer
// ---------------------------------------------------------------------------

/// A reusable heap buffer with a fixed capacity and a mutable populated-byte
/// count.
///
/// [`BufferPool`] owns a set of these and circulates them between the
/// decompressor and writer threads.  `size` tracks how many leading bytes of
/// `data` are currently valid; it must be reset to `0` before returning a
/// buffer to the pool.
#[derive(Debug)]
pub struct Buffer {
    /// The underlying storage.
    pub data: Vec<u8>,
    /// Number of valid bytes currently held in `data`.
    pub size: usize,
}

impl Buffer {
    fn new(capacity: usize) -> Self {
        Buffer {
            data: vec![0u8; capacity],
            size: 0,
        }
    }

    /// Capacity of the buffer in bytes.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.data.capacity()
    }

    /// Returns the populated slice of the buffer.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.data[..self.size]
    }

    /// Returns a mutable reference to the full backing storage (up to capacity).
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        let cap = self.data.capacity();
        &mut self.data[..cap]
    }
}

// ---------------------------------------------------------------------------
// BufferPool
// ---------------------------------------------------------------------------

/// A fixed-size pool of reusable [`Buffer`] objects for multi-threaded
/// decompression.
///
/// Buffers are passed through a `crossbeam_channel::bounded` channel.
/// [`acquire`][BufferPool::acquire] blocks until a buffer is available;
/// [`release`][BufferPool::release] returns one.  This gives FIFO ordering
/// and bounded parallelism without busy-waiting.
pub struct BufferPool {
    sender: Sender<Buffer>,
    receiver: Receiver<Buffer>,
}

impl BufferPool {
    /// Creates a pool pre-filled with `count` buffers each of `buf_size`
    /// bytes.
    ///
    /// Pass [`PBUFFERS_NB`] as `count` for the standard three-buffer pipeline
    /// (one being decompressed, one queued, one in-flight to I/O).
    ///
    /// # Panics
    ///
    /// Panics if `count` is 0.
    pub fn new(buf_size: usize, count: usize) -> Self {
        assert!(count > 0, "BufferPool count must be > 0");
        let (sender, receiver) = bounded(count);
        for _ in 0..count {
            sender
                .send(Buffer::new(buf_size))
                .expect("channel capacity matches loop count");
        }
        BufferPool { sender, receiver }
    }

    /// Acquires a buffer from the pool, blocking until one is available.
    pub fn acquire(&self) -> Buffer {
        self.receiver
            .recv()
            .expect("BufferPool channel closed unexpectedly")
    }

    /// Returns a buffer to the pool.
    ///
    /// The caller must reset `buf.size` to `0` before releasing so that the
    /// next acquirer sees a logically empty buffer.
    pub fn release(&self, buf: Buffer) {
        self.sender
            .send(buf)
            .expect("BufferPool channel closed unexpectedly");
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_new_zeroed() {
        let b = Buffer::new(1024);
        assert_eq!(b.capacity(), 1024);
        assert_eq!(b.size, 0);
        assert_eq!(b.as_slice().len(), 0);
    }

    #[test]
    fn buffer_pool_acquire_release() {
        let pool = BufferPool::new(4096, PBUFFERS_NB);
        let mut b = pool.acquire();
        assert_eq!(b.capacity(), 4096);
        b.size = 0;
        pool.release(b);
        // We should be able to acquire again without blocking.
        let b2 = pool.acquire();
        assert_eq!(b2.capacity(), 4096);
        pool.release(b2);
    }

    #[test]
    fn buffer_pool_exhaustion_and_restore() {
        let pool = BufferPool::new(256, 2);
        let b1 = pool.acquire();
        let b2 = pool.acquire();
        // Pool is now empty. Release both back.
        pool.release(b1);
        pool.release(b2);
        // Should be able to acquire again.
        let b3 = pool.acquire();
        pool.release(b3);
    }

    #[test]
    fn buffer_pool_concurrent_acquire_release() {
        use std::sync::Arc;
        use std::thread;

        let pool = Arc::new(BufferPool::new(1024, PBUFFERS_NB));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let p = Arc::clone(&pool);
            handles.push(thread::spawn(move || {
                let mut buf = p.acquire();
                // Simulate work.
                buf.size = 10;
                buf.size = 0;
                p.release(buf);
            }));
        }
        for h in handles {
            h.join().expect("thread panicked");
        }
    }

    #[test]
    fn decompress_resources_new() {
        let prefs = Prefs::default();
        let res = DecompressResources::new(&prefs).expect("should not fail");
        assert_eq!(res.src_buffer.len(), LZ4IO_D_BUFFER_SIZE);
        assert_eq!(res.dst_buffer.len(), LZ4IO_D_BUFFER_SIZE);
        assert!(res.dict_buffer.is_none());
    }

    #[test]
    fn decompress_resources_from_prefs_no_dict() {
        let prefs = Prefs::default();
        let res = DecompressResources::from_prefs(&prefs).expect("no dict, should succeed");
        assert!(res.dict().is_none());
    }

    #[test]
    fn load_dict_file_small() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        let data = b"hello world dictionary content";
        tmp.write_all(data).unwrap();
        let dict = load_dict_file(tmp.path()).expect("load should succeed");
        assert_eq!(dict, data.as_ref());
    }

    #[test]
    fn load_dict_file_large_truncated_to_64k() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        // Write 128 KiB of incrementing bytes; only the last 64 KiB should be kept.
        let data: Vec<u8> = (0u8..=255).cycle().take(128 * 1024).collect();
        tmp.write_all(&data).unwrap();
        let dict = load_dict_file(tmp.path()).expect("load should succeed");
        assert_eq!(dict.len(), LZ4_MAX_DICT_SIZE);
        // Last 64 KiB of data must match.
        assert_eq!(dict, &data[64 * 1024..]);
    }
}
