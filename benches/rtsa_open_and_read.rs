//! Throughput benchmark for `RtsaSource::open` + `read_samples`.
//!
//! Uses the bundled CW IQ Git-LFS fixture (`tests/IQ-Sample-Data-CW-…rtsa`).
//! When the LFS content hasn't been pulled — common in fresh clones,
//! CI without LFS, or sandboxed environments — the bench prints a
//! skip notice and exits successfully so `cargo bench` doesn't fail
//! a release pipeline on missing fixtures.
//!
//! Run with:
//!
//! ```bash
//! git lfs pull
//! cargo bench --bench rtsa_open_and_read
//! ```

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use sdr_aaronia_rs::file_source::RtsaSource;
use std::path::PathBuf;

fn cw_iq_capture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("IQ-Sample-Data-CW-2410MHz-1MHzSampleRate.rtsa")
}

/// Returns `true` when the LFS fixture hasn't been pulled. Mirrors the
/// helper in `tests/integration_test.rs::rtsa_fixture_missing`.
fn fixture_missing(path: &PathBuf) -> bool {
    if !path.exists() {
        return true;
    }
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return true,
    };
    if meta.len() < 1024 {
        return true;
    }
    use std::io::Read;
    if let Ok(mut f) = std::fs::File::open(path) {
        let mut buf = [0u8; 16];
        if f.read(&mut buf)
            .map(|n| &buf[..n] == b"version https://")
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

fn bench_open_and_read(c: &mut Criterion) {
    let path = cw_iq_capture_path();
    if fixture_missing(&path) {
        eprintln!(
            "skipping rtsa_open_and_read bench: {} is missing or an LFS pointer (run `git lfs pull`)",
            path.display()
        );
        return;
    }

    // Verify we can open the file. If decompression fails due to missing
    // proprietary tooling, skip gracefully instead of panicking the benchmark runner.
    if let Err(e) = RtsaSource::open(&path) {
        let err_msg = format!("{:?}", e);
        if err_msg.contains("RTSAFileTool was not found") {
            eprintln!(
                "skipping rtsa_open_and_read bench: RTSAFileTool not found (Aaronia RTSA-Suite PRO required for decompression)."
            );
            return;
        }
        panic!("unexpected error opening fixture: {}", e);
    }

    let mut group = c.benchmark_group("rtsa_open_and_read");
    // Reduce sample count — opening + reading a 4.4 MB file is
    // expensive (mmap + chunk walk + slice copy). Default 100 samples
    // per iteration would make the bench take minutes.
    group.sample_size(20);

    group.bench_function("open", |b| {
        b.iter(|| {
            let source = RtsaSource::open(black_box(&path)).expect("open should succeed");
            black_box(source);
        });
    });

    group.bench_function("open_and_read_1024", |b| {
        b.iter(|| {
            let mut source = RtsaSource::open(black_box(&path)).expect("open should succeed");
            let data = source
                .read_samples(1024, None)
                .expect("read should succeed");
            black_box(data);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_open_and_read);
criterion_main!(benches);
