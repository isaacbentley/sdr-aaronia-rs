//! Throughput benchmark for `utils::deinterleave_dual_iq` — the
//! dual-channel demux on the native-SDK read path's per-packet hot
//! loop. The function's contract includes being allocation-free (it
//! returns a borrowed iterator); this bench pins its cost per packet
//! so a regression back to an allocating implementation, or a
//! bounds-check pessimization, shows up as a step change.
//!
//! Run with:
//!
//! ```bash
//! cargo bench --bench deinterleave_dual_iq
//! ```

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use sdr_aaronia_rs::utils::deinterleave_dual_iq;

/// One SDK packet's worth of samples. Real `spectranv6/raw` packets
/// observed on hardware carry tens of thousands of samples; 16384 is a
/// representative mid-size packet.
const SAMPLES_PER_PACKET: usize = 16 * 1024;

fn bench_deinterleave(c: &mut Criterion) {
    let mut group = c.benchmark_group("deinterleave_dual_iq");

    // stride 4 = tightly packed [I1 Q1 I2 Q2]; stride 6 = two padding
    // floats per sample, the other layout the demux must handle.
    for stride in [4usize, 6] {
        let floats = vec![1.0f32; (SAMPLES_PER_PACKET - 1) * stride + 4];
        group.throughput(Throughput::Elements(SAMPLES_PER_PACKET as u64));
        group.bench_with_input(BenchmarkId::new("stride", stride), &stride, |b, &stride| {
            b.iter(|| {
                let mut acc = (0.0f32, 0.0f32);
                for (a, bb) in deinterleave_dual_iq(black_box(&floats), SAMPLES_PER_PACKET, stride)
                    .expect("valid layout")
                {
                    acc.0 += a.re;
                    acc.1 += bb.im;
                }
                black_box(acc)
            })
        });
    }

    group.finish();
}

criterion_group!(benches, bench_deinterleave);
criterion_main!(benches);
