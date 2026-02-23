/// Returns compressible synthetic data of the given size.
///
/// The output is a Latin-like lorem-ipsum string repeated to fill exactly
/// `size` bytes.  Because it is highly repetitive, LZ4 compresses it well,
/// giving throughput numbers that reflect the codec rather than the data.
pub fn synthetic_data(size: usize) -> Vec<u8> {
    const LOREM: &[u8] = b"Lorem ipsum dolor sit amet, consectetur adipiscing elit, \
        sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. \
        Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi \
        ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit \
        in voluptate velit esse cillum dolore eu fugiat nulla pariatur. \
        Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia \
        deserunt mollit anim id est laborum. ";

    let mut out = Vec::with_capacity(size);
    while out.len() < size {
        let rem = size - out.len();
        let take = rem.min(LOREM.len());
        out.extend_from_slice(&LOREM[..take]);
    }
    out
}

/// Returns benchmark corpus chunks, each of exactly `chunk_size` bytes.
///
/// If the environment variable `SILESIA_CORPUS_DIR` is set, files are read
/// from that directory and padded / truncated to `chunk_size`.  Otherwise
/// three synthetic chunks are returned so that `cargo bench` always works
/// without any external corpus present.
#[allow(dead_code)]
pub fn corpus_chunks(chunk_size: usize) -> Vec<Vec<u8>> {
    use std::io::Read;

    if let Ok(dir) = std::env::var("SILESIA_CORPUS_DIR") {
        let mut chunks = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                if let Ok(mut f) = std::fs::File::open(&path) {
                    let mut buf = Vec::new();
                    if f.read_to_end(&mut buf).is_ok() && !buf.is_empty() {
                        let chunk = if buf.len() >= chunk_size {
                            buf[..chunk_size].to_vec()
                        } else {
                            // Pad by repeating the file content.
                            let mut c = buf.clone();
                            while c.len() < chunk_size {
                                let rem = chunk_size - c.len();
                                let take = rem.min(buf.len());
                                c.extend_from_slice(&buf[..take]);
                            }
                            c
                        };
                        chunks.push(chunk);
                    }
                }
            }
        }
        if !chunks.is_empty() {
            return chunks;
        }
    }

    // Synthetic fallback — always works, no external files required.
    vec![
        synthetic_data(chunk_size),
        synthetic_data(chunk_size),
        synthetic_data(chunk_size),
    ]
}
