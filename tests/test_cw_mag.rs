use sdr_aaronia_rs::file_source::{RtsaSource, SampleData};

fn fixture_missing(path: &std::path::Path) -> bool {
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
    if let Ok(mut f) = std::fs::File::open(path) {
        use std::io::Read;
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

#[test]
fn test_cw_magnitudes() {
    let path = std::path::Path::new("tests/IQ-Sample-Data-CW-2410MHz-1MHzSampleRate.rtsa");
    if fixture_missing(path) {
        println!("skipping test_cw_magnitudes: CW fixture is missing or an LFS pointer");
        return;
    }

    let mut source = match RtsaSource::open(path) {
        Ok(s) => s,
        Err(e) if e.to_string().contains("RTSAFileTool was not found") => {
            println!(
                "skipping test_cw_magnitudes: RTSAFileTool not found to decompress CW fixture"
            );
            return;
        }
        Err(e) => panic!("open failed: {}", e),
    };
    let data = source.read_samples(10240, None).unwrap().unwrap();
    let samples = match data {
        SampleData::Iq(s) => s,
        _ => panic!(),
    };

    assert!(!samples.is_empty(), "CW fixture should yield IQ samples");

    // A continuous-wave capture should be overwhelmingly non-zero and
    // every magnitude must be finite.
    let mut non_zero = 0usize;
    for s in &samples {
        let mag = (s.re * s.re + s.im * s.im).sqrt();
        assert!(mag.is_finite(), "sample magnitude must be finite");
        if mag > 0.0 {
            non_zero += 1;
        }
    }
    assert!(
        non_zero * 2 > samples.len(),
        "CW capture should be mostly non-zero: {} of {} samples had energy",
        non_zero,
        samples.len()
    );
}
