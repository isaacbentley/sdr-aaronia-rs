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
fn test_cw_meta() {
    let path = std::path::Path::new("tests/IQ-Sample-Data-CW-2410MHz-1MHzSampleRate.rtsa");
    if fixture_missing(path) {
        println!("skipping test_cw_meta: CW fixture is missing or an LFS pointer");
        return;
    }

    let source = match RtsaSource::open(path) {
        Ok(s) => s,
        Err(e) if e.to_string().contains("RTSAFileTool was not found") => {
            println!("skipping test_cw_meta: RTSAFileTool not found to decompress CW fixture");
            return;
        }
        Err(e) => panic!("open failed: {}", e),
    };
    // The fixture is a CW tone at 2410 MHz captured at 1 MSPS; the parsed
    // metadata must reflect that tuning.
    let meta = source.metadata();
    let center = meta
        .center_frequency
        .expect("CW fixture must report a center frequency");
    assert!(
        (center - 2_410_000_000.0).abs() < 1_000.0,
        "expected ~2410 MHz center, got {center} Hz"
    );
    assert!(
        (meta.sample_rate - 1_000_000.0).abs() < 1_000.0,
        "expected ~1 MSPS sample rate, got {} Hz",
        meta.sample_rate
    );
    assert!(meta.total_samples > 0, "fixture must report sample count");

    // Reading a chunk must produce IQ samples.
    let mut s2 = RtsaSource::open(path).unwrap();
    let data = s2.read_samples(1024, None).unwrap().unwrap();
    match data {
        SampleData::Iq(samples) => assert!(!samples.is_empty(), "expected non-empty IQ read"),
        other => panic!("expected IQ samples from CW fixture, got {other:?}"),
    }
}
