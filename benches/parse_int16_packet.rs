//! Throughput benchmark for the HTTP-stream int16 IQ parser.
//!
//! Measures wall-clock cost of `StreamParser::process_data` on a single
//! synthetic int16 packet containing N IQ pairs. Run with:
//!
//! ```bash
//! cargo bench --bench parse_int16_packet
//! ```
//!
//! Criterion writes HTML reports under `target/criterion/` and saves a
//! `base` baseline you can compare against later runs:
//!
//! ```bash
//! cargo bench --bench parse_int16_packet -- --save-baseline pre
//! # ... make a change ...
//! cargo bench --bench parse_int16_packet -- --baseline pre
//! ```

use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use sdr_aaronia_rs::http_streaming::{StreamFormat, StreamParser};

/// Build one int16 packet: JSON header + 0x1E + `samples` IQ pairs.
/// Each IQ pair is 4 bytes (2 × i16). `scale` ends up in `StreamParser`
/// rather than the JSON, matching how the server-side `?scale=` URL
/// parameter actually arrives.
fn build_int16_packet(samples: usize) -> Bytes {
    let json = format!(
        r#"{{"startTime":0.0,"endTime":1.0,"startFrequency":0.0,"endFrequency":1.0,"samples":{},"unit":"volt","payload":"iq","minPower":0,"maxPower":1,"sampleSize":1}}"#,
        samples
    );
    let mut buf = json.into_bytes();
    buf.push(0x1E);
    // Deterministic sample data — values chosen so the bench doesn't
    // accidentally hit the framing edge case (already fixed, but we
    // keep the values varied for realism).
    for i in 0..samples {
        let re = (i as i16).wrapping_mul(37);
        let im = (i as i16).wrapping_mul(-41);
        buf.extend_from_slice(&re.to_le_bytes());
        buf.extend_from_slice(&im.to_le_bytes());
    }
    Bytes::from(buf)
}

fn bench_parse_int16(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse_int16_packet");
    // Realistic packet sizes from RTSA Suite HTTP streaming. 1024
    // samples ≈ 1 ms at 1 MS/s, 65536 ≈ 1 ms at 65 MS/s.
    for samples in [256usize, 1024, 8192, 65_536] {
        let packet = build_int16_packet(samples);
        // Each IQ pair is 4 bytes → throughput in bytes per sec.
        group.throughput(Throughput::Bytes((samples * 4) as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(samples),
            &packet,
            |b, packet| {
                b.iter(|| {
                    // Fresh parser each iter to avoid amortising the
                    // single allocation cost across many runs (more
                    // representative of "open connection, parse first
                    // packet" than "warmed-up stream").
                    let mut parser =
                        StreamParser::new(StreamFormat::Int16, Some(1.0 / 32768.0)).unwrap();
                    let result = parser.process_data(black_box(packet)).unwrap();
                    black_box(result);
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_parse_int16);
criterion_main!(benches);
