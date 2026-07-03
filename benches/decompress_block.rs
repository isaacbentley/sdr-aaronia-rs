//! Throughput benchmark for the Rice + inverse-wavelet decompressor.
//!
//! Measures decode cost on a fixed deterministic byte buffer. The
//! `Decompressor` accepts arbitrary bytes as Rice-coded coefficients
//! (the codec is permissive on input), so we can drive it from a
//! pseudo-random buffer without first having to encode a "real" block.
//! That gives us a stable, reproducible benchmark even without an
//! Aaronia fixture in CI.
//!
//! Run with:
//!
//! ```bash
//! cargo bench --bench decompress_block
//! ```

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use sdr_aaronia_rs::decompression::Decompressor;

/// Linear congruential generator. Stable across platforms and
/// Rust versions, so the byte sequence the bench feeds the
/// decompressor is identical for every run.
fn lcg_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        out.push((state >> 33) as u8);
    }
    out
}

fn bench_decompress(c: &mut Criterion) {
    let dec = Decompressor::new();
    let mut group = c.benchmark_group("decompress_block");

    // Block sizes that bracket realistic spectrum payloads. Aaronia
    // typically uses ~1024-16384 frequency bins per spectrum.
    for &(rows, cols) in &[(16usize, 64usize), (32, 256), (64, 1024)] {
        let payload_bytes = rows * cols * 4; // rough upper bound for Rice-coded i32 coefficients
        let data = lcg_bytes(payload_bytes, 0xA0A0A0A0A0A0A0A0);
        group.throughput(Throughput::Bytes(payload_bytes as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}x{}", rows, cols)),
            &(rows, cols, data),
            |b, (rows, cols, data)| {
                b.iter(|| {
                    // Compression factor 1 is the lightest non-zero
                    // factor (factor 0 is the rejected-early case).
                    let result = dec.decompress(black_box(data), 1, *rows, *cols);
                    black_box(result.ok());
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_decompress);
criterion_main!(benches);
