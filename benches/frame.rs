//! Criterion benchmarks for the LZ4 frame-format compression API.
//!
//! Run with:
//!   cargo bench --bench frame
//!
//! Optionally set SILESIA_CORPUS_DIR for real-world corpus data.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

mod corpus {
    include!("corpus.rs");
}

fn bench_frame_compress_decompress(c: &mut Criterion) {
    let mut group = c.benchmark_group("frame_compress_decompress");

    for &chunk_size in &[65_536usize, 262_144, 4_194_304] {
        // Use real corpus data when SILESIA_CORPUS_DIR is set, else synthetic.
        let chunks = corpus::corpus_chunks(chunk_size);
        let chunk = chunks[0].clone();
        let prefs = lz4::frame::Preferences::default();

        // ── lz4f_compress_frame ──────────────────────────────────────────────
        {
            let bound = lz4::frame::lz4f_compress_frame_bound(chunk_size, Some(&prefs));
            let mut dst = vec![0u8; bound];

            group.throughput(Throughput::Bytes(chunk_size as u64));
            group.bench_with_input(
                BenchmarkId::new("lz4f_compress_frame", chunk_size),
                &chunk,
                |b, chunk| {
                    b.iter(|| {
                        lz4::frame::lz4f_compress_frame(&mut dst, chunk, Some(&prefs)).unwrap()
                    })
                },
            );

            // Build the pre-compressed buffer for the decompress bench below.
            let n =
                lz4::frame::lz4f_compress_frame(&mut dst, &chunk, Some(&prefs)).unwrap();
            let compressed = dst[..n].to_vec();

            // ── lz4f_decompress (streaming) ──────────────────────────────────
            // A single 4 MiB output buffer is reused across iterations.
            // The decompression context is reset at the start of every
            // iteration so we only measure the decode work, not allocation.
            let mut out_buf = vec![0u8; chunk_size + 65_536];
            let mut dctx =
                lz4::frame::lz4f_create_decompression_context(100).unwrap();

            group.throughput(Throughput::Bytes(chunk_size as u64));
            group.bench_with_input(
                BenchmarkId::new("lz4f_decompress", chunk_size),
                &compressed,
                |b, compressed| {
                    b.iter(|| {
                        lz4::frame::lz4f_reset_decompression_context(&mut dctx);
                        let mut pos = 0usize;
                        while pos < compressed.len() {
                            let (consumed, _written, hint) = lz4::frame::lz4f_decompress(
                                &mut dctx,
                                Some(&mut out_buf),
                                &compressed[pos..],
                                None,
                            )
                            .unwrap();
                            pos += consumed;
                            if hint == 0 || (consumed == 0 && _written == 0) {
                                break;
                            }
                        }
                    })
                },
            );
        }
    }

    group.finish();
}

criterion_group!(benches, bench_frame_compress_decompress);
criterion_main!(benches);
