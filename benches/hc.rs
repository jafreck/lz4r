//! Criterion benchmarks for the LZ4 HC (high-compression) block API.
//!
//! Run with:
//!   cargo bench --bench hc
//!
//! Optionally set SILESIA_CORPUS_DIR for real-world corpus data.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

mod corpus {
    include!("corpus.rs");
}

fn bench_hc_compress(c: &mut Criterion) {
    let mut group = c.benchmark_group("hc_compress");

    // Use a 256 KiB chunk — representative of typical streaming block sizes.
    let chunk_size = 262_144usize;
    let chunks = corpus::corpus_chunks(chunk_size);
    let chunk = chunks[0].clone();
    let bound = lz4::compress_bound(chunk_size as i32).max(0) as usize;

    for &level in &[1i32, 4, 8, 12] {
        let mut dst = vec![0u8; bound];

        group.throughput(Throughput::Bytes(chunk_size as u64));
        group.bench_with_input(
            BenchmarkId::new("compress_hc", level),
            &chunk,
            |b, chunk| {
                b.iter(|| {
                    let n = unsafe {
                        lz4::hc::compress_hc(
                            chunk.as_ptr(),
                            dst.as_mut_ptr(),
                            chunk.len() as i32,
                            dst.len() as i32,
                            level,
                        )
                    };
                    assert!(n > 0, "compress_hc returned 0");
                    n
                })
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_hc_compress);
criterion_main!(benches);
