//! Criterion benchmarks for the LZ4 block (raw) compression API.
//!
//! Run with:
//!   cargo bench --bench block
//!
//! Optionally set SILESIA_CORPUS_DIR to a directory of corpus files so the
//! benchmarks run against real-world data instead of synthetic lorem ipsum.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

mod corpus {
    include!("corpus.rs");
}

fn bench_block_compress_decompress(c: &mut Criterion) {
    let mut group = c.benchmark_group("block_compress_decompress");

    for &chunk_size in &[65_536usize, 262_144] {
        // Use real corpus chunks when SILESIA_CORPUS_DIR is set, else synthetic.
        let chunks = corpus::corpus_chunks(chunk_size);
        let chunk = chunks[0].clone();
        let bound = lz4::compress_bound(chunk_size as i32).max(0) as usize;

        // ── compress_default ────────────────────────────────────────────────
        {
            let mut dst = vec![0u8; bound];
            group.throughput(Throughput::Bytes(chunk_size as u64));
            group.bench_with_input(
                BenchmarkId::new("compress_default", chunk_size),
                &chunk,
                |b, chunk| {
                    b.iter(|| lz4::block::compress_default(chunk, &mut dst).unwrap())
                },
            );
        }

        // ── compress_fast with several acceleration factors ──────────────────
        for &acc in &[1i32, 3, 9] {
            let mut dst = vec![0u8; bound];
            group.throughput(Throughput::Bytes(chunk_size as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("compress_fast_{acc}"), chunk_size),
                &chunk,
                |b, chunk| {
                    b.iter(|| lz4::block::compress_fast(chunk, &mut dst, acc).unwrap())
                },
            );
        }

        // ── decompress_safe — pre-compress the chunk once, then benchmark ───
        {
            let mut tmp = vec![0u8; bound];
            let n = lz4::block::compress_default(&chunk, &mut tmp).unwrap();
            let compressed = tmp[..n].to_vec();
            let mut decomp_dst = vec![0u8; chunk_size];

            // Throughput measured in *decompressed* bytes (the meaningful quantity).
            group.throughput(Throughput::Bytes(chunk_size as u64));
            group.bench_with_input(
                BenchmarkId::new("decompress_safe", chunk_size),
                &compressed,
                |b, compressed| {
                    b.iter(|| lz4::block::decompress_safe(compressed, &mut decomp_dst).unwrap())
                },
            );
        }
    }

    group.finish();
}

criterion_group!(benches, bench_block_compress_decompress);
criterion_main!(benches);
