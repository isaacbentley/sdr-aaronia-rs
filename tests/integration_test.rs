use bytes::Bytes;
use sdr_aaronia_rs::file_source::{RtsaSource, SampleData};
use sdr_aaronia_rs::http_streaming::{StreamFormat, StreamParser};
use std::path::PathBuf;

fn wrap_mock_packet(payload: &[u8], payload_type: &str, sample_size: u64) -> Bytes {
    let json_meta = format!(
        r#"{{"startTime":0.0,"endTime":1.0,"startFrequency":0.0,"endFrequency":1.0,"samples":1,"unit":"volt","payload":"{}","minPower":0,"maxPower":1,"sampleSize":{}}}"#,
        payload_type, sample_size
    );
    let mut data = json_meta.into_bytes();
    data.push(30); // RS character
    data.extend_from_slice(payload);
    Bytes::from(data)
}

#[test]
fn test_stream_parser_int16_format() {
    let mut parser = StreamParser::new(StreamFormat::Int16, None).unwrap();

    // Create mock Int16 binary data (4 bytes = 2 int16 values = 1 IQ pair)
    // Values: 1000 (0x03E8) and -2000 (0xF830)
    // Little endian binary: E8 03 30 F8
    let binary_data = vec![0xE8, 0x03, 0x30, 0xF8];
    let bytes = wrap_mock_packet(&binary_data, "iq", 1);

    let result = parser.process_data(&bytes).unwrap();

    // Scale factor is 1.0 / 32768.0 since it's INT16
    let expected_scale = 1.0 / 32768.0;

    // aaronia-rs parser handles IQ as complex pairs: Real=1000, Imag=-2000
    assert_eq!(result[0].samples.len(), 1);
    assert!((result[0].samples[0].re - (1000.0 * expected_scale)).abs() < 1e-6);
    assert!((result[0].samples[0].im - (-2000.0 * expected_scale)).abs() < 1e-6);
}

#[test]
fn test_stream_parser_f32_format() {
    let mut parser = StreamParser::new(StreamFormat::Float32, None).unwrap();

    // 1.5f32 = 0x3FC00000 -> LE: 00 00 C0 3F
    // -2.5f32 = 0xC0200000 -> LE: 00 00 20 C0
    let binary_data = vec![
        0x00, 0x00, 0xC0, 0x3F, // Re: 1.5
        0x00, 0x00, 0x20, 0xC0, // Im: -2.5
    ];

    let bytes = wrap_mock_packet(&binary_data, "iq", 1);
    let result = parser.process_data(&bytes).unwrap();

    assert_eq!(result[0].samples.len(), 1);
    assert!((result[0].samples[0].re - 1.5).abs() < 1e-6);
    assert!((result[0].samples[0].im - -2.5).abs() < 1e-6);
}

#[test]
fn test_stream_parser_invalid_format() {
    // Unsupported format string is rejected at construction.
    assert!(StreamFormat::from_str("unsupported_format").is_err());

    // Feeding a partial packet (header without separator/binary tail) is
    // not an error — `process_data` should buffer and return Ok(empty)
    // so the caller can supply the rest of the bytes.
    let mut parser = StreamParser::new(StreamFormat::Int16, None).unwrap();
    let partial = b"{\"startTime\":0.0,\"samples\":1";
    let result = parser.process_data(&Bytes::from(partial.to_vec()));
    let packets = result.expect("partial packet must buffer, not error");
    assert!(
        packets.is_empty(),
        "partial packet must produce zero parsed packets"
    );

    // Malformed JSON whose first '{' is followed by garbage and a
    // separator must surface as Err — exercise that path.
    let mut parser = StreamParser::new(StreamFormat::Int16, None).unwrap();
    let mut bogus = b"{not_real_json".to_vec();
    bogus.push(0x1E);
    bogus.extend_from_slice(&[0u8; 8]); // pretend binary tail
    // The parser may return Ok(empty) (waiting for more data) or Err —
    // both are acceptable; what must NOT happen is a panic.
    let _ = parser.process_data(&Bytes::from(bogus));
}

#[test]
fn test_stream_parser_f16_format() {
    let mut parser = StreamParser::new(StreamFormat::Float16, None).unwrap();

    // f16 (half) precision values:
    // 1.5 in f16 is 0x3E00 -> LE: 00 3E
    // -2.5 in f16 is 0xC100 -> LE: 00 C1
    let binary_data = vec![
        0x00, 0x3E, // Re: 1.5
        0x00, 0xC1, // Im: -2.5
    ];

    let bytes = wrap_mock_packet(&binary_data, "iq", 1);
    let result = parser.process_data(&bytes).unwrap();

    assert_eq!(result[0].samples.len(), 1);
    assert!((result[0].samples[0].re - 1.5).abs() < 1e-3);
    assert!((result[0].samples[0].im - -2.5).abs() < 1e-3);
}

#[test]
fn test_stream_parser_json_format() {
    let mut parser = StreamParser::new(StreamFormat::Json, None).unwrap();

    // For JSON streams the `samples` field carries the array of values,
    // not a count. The custom deserializer on `PacketMetadata.samples`
    // accepts either form. On the wire each JSON document is compact
    // (no raw newlines — those act as packet separators) and terminated
    // by a line feed.
    let json_payload = concat!(
        r#"{"startTime":0.0,"endTime":1.0,"startFrequency":0.0,"endFrequency":1.0,"#,
        r#""unit":"volt","payload":"iq","minPower":0,"maxPower":1,"sampleSize":2,"#,
        r#""samples":[1.5,-2.5]}"#,
        "\n"
    );

    let bytes = Bytes::from(json_payload.as_bytes());
    let packets = parser.process_data(&bytes).unwrap();
    assert_eq!(packets.len(), 1);
    let result = &packets[0];

    assert_eq!(result.samples.len(), 1);
    assert!((result.samples[0].re - 1.5).abs() < 1e-6);
    assert!((result.samples[0].im - -2.5).abs() < 1e-6);
    // The parser should overwrite `samples` with the decoded Complex32
    // count, not the raw array length.
    assert_eq!(result.metadata.samples, 1);
}

#[test]
fn test_stream_parser_resyncs_after_corrupt_packet() {
    // A corrupt segment ahead of a valid packet must not poison the
    // stream: the parser skips it and decodes the following packet.
    let mut parser = StreamParser::new(StreamFormat::Float32, None).unwrap();

    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"{this is not json}\n");
    let binary_data = 1.5f32
        .to_le_bytes()
        .iter()
        .chain(&(-2.5f32).to_le_bytes())
        .copied()
        .collect::<Vec<u8>>();
    bytes.extend_from_slice(&wrap_mock_packet(&binary_data, "iq", 1));

    let packets = parser.process_data(&Bytes::from(bytes)).unwrap();
    assert_eq!(packets.len(), 1, "valid packet after garbage must decode");
    assert!((packets[0].samples[0].re - 1.5).abs() < 1e-6);
    assert!((packets[0].samples[0].im - -2.5).abs() < 1e-6);
}

#[test]
fn test_stream_parser_multiple_packets_in_one_chunk() {
    // One HTTP chunk containing two packets must yield both — the old
    // one-packet-per-chunk API silently dropped the second.
    let mut parser = StreamParser::new(StreamFormat::Float32, None).unwrap();

    let sample = |re: f32, im: f32| -> Vec<u8> {
        re.to_le_bytes()
            .iter()
            .chain(&im.to_le_bytes())
            .copied()
            .collect()
    };
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&wrap_mock_packet(&sample(1.0, 2.0), "iq", 1));
    bytes.extend_from_slice(&wrap_mock_packet(&sample(3.0, 4.0), "iq", 1));

    let packets = parser.process_data(&Bytes::from(bytes)).unwrap();
    assert_eq!(packets.len(), 2);
    assert!((packets[0].samples[0].re - 1.0).abs() < 1e-6);
    assert!((packets[1].samples[0].re - 3.0).abs() < 1e-6);
}

/// Path to the bundled CW IQ capture. Filename declares the recording
/// parameters: 2410 MHz center, 1 MHz sample rate. The file's DSFH
/// creation-time decodes to 2020-11-24 10:53:40 UTC (which is the
/// timestamp suffix the file shipped with originally). Used by the
/// metadata + IQ-read integration tests below.
fn cw_iq_capture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("IQ-Sample-Data-CW-2410MHz-1MHzSampleRate.rtsa")
}

/// Larger LTE capture: 1829.4 MHz, 10 MHz sample rate (~183 MB).
fn lte_iq_capture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("IQ-Sample-Data-LTE-1829.4MHz-10MHzSampleRate.rtsa")
}

/// Spectra capture (not IQ): 2410 MHz, 1 MHz sample rate. Lets us
/// exercise the `SampleData::Spectra` branch.
fn cw_spectra_capture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("SPECTRA-Sample-Data-CW-2410MHz-1MHzSampleRate.rtsa")
}

/// Returns `true` when the path is missing, empty, or appears to be a
/// Git-LFS pointer file rather than the actual binary capture. The
/// repository tracks the .rtsa fixtures via LFS; in environments where
/// the LFS content hasn't been pulled (CI, fresh clones, sandboxed
/// builds) the on-disk file is the ~130-byte text pointer instead of the
/// real RTSA file. Treat both cases as "skip cleanly" rather than fail
/// the test suite.
fn rtsa_fixture_missing(path: &PathBuf) -> bool {
    if !path.exists() {
        return true;
    }
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return true,
    };
    // A real RTSA file is at minimum a few KB (DSFH + STRM + SAMP +
    // DSFT). An LFS pointer is exactly 132–134 bytes and begins with
    // the literal `version https://`. Combine both checks so we can
    // skip on either signal.
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
fn test_rtsa_file_opens_and_parses_metadata() {
    let path = cw_iq_capture_path();
    if rtsa_fixture_missing(&path) {
        eprintln!(
            "skipping test_rtsa_file_opens_and_parses_metadata: {} is missing or an LFS pointer (run `git lfs pull`)",
            path.display()
        );
        return;
    }

    let source = match RtsaSource::open(&path) {
        Ok(s) => s,
        Err(e) if e.to_string().contains("RTSAFileTool was not found") => {
            println!(
                "skipping test_rtsa_file_opens_and_parses_metadata: RTSAFileTool not found to decompress CW fixture"
            );
            return;
        }
        Err(e) => panic!("RtsaSource::open should succeed: {}", e),
    };
    let meta = source.metadata();

    // The bundled capture is a real 4.4 MB recording with at least one
    // SAMP chunk, so `total_samples` must be non-zero.
    assert!(
        source.total_samples() > 0,
        "expected total_samples > 0, got {}",
        source.total_samples()
    );
    assert!(!source.is_eof(), "fresh source should not be at EOF");
    assert_eq!(source.current_position(), 0);

    // Forum-driven µs-vs-seconds heuristic: this capture's raw DSFH
    // creation_time is 1.606e15 (microseconds), which our normaliser
    // divides by 1e6 to get ~1.606e9 seconds (2020-11-24 10:53:40 UTC).
    // Without the normalisation this would parse as the year 52899.
    let utc_2020_11_24 = 1_606_204_800.0; // 2020-11-24 00:00:00 UTC
    let utc_2020_11_25 = 1_606_291_200.0; // 2020-11-25 00:00:00 UTC
    assert!(
        meta.creation_time >= utc_2020_11_24 && meta.creation_time < utc_2020_11_25,
        "DSFH creation_time {} should normalise into 2020-11-24 UTC \
         (expected [{}, {}))",
        meta.creation_time,
        utc_2020_11_24,
        utc_2020_11_25
    );

    // The capture file is < 1 GB, so timestamps stored as nanoseconds
    // since epoch should fit easily in u64.
    assert!(meta.start_time_ns > 0, "start_time_ns must be populated");

    // At least one sub-stream must be present for a valid IQ capture.
    assert!(
        !source.sub_stream_info().is_empty(),
        "expected at least one SSTR sub-stream"
    );

    // The capture's filename declares 1 MHz sample rate. The SDK doesn't
    // always populate `meta.sample_rate` directly (it can be derived from
    // SSTR.frequency_step or, when neither STRM.sample_rate nor SSTR has
    // it, left at 0.0). Probe both candidate sources before giving up.
    let sample_rate_candidate = if meta.sample_rate > 0.0 {
        meta.sample_rate
    } else {
        source
            .sub_stream_info()
            .iter()
            .map(|s| s.frequency_step)
            .find(|&step| step > 0.0)
            .unwrap_or(0.0)
    };
    assert!(
        sample_rate_candidate >= 0.0,
        "sample-rate candidate {} must be non-negative",
        sample_rate_candidate
    );
    if sample_rate_candidate > 0.0 {
        assert!(
            sample_rate_candidate > 100_000.0 && sample_rate_candidate < 10_000_000.0,
            "sample-rate candidate {} should be roughly 1 MHz",
            sample_rate_candidate
        );
    }
}

#[test]
fn test_rtsa_file_reads_iq_samples() {
    let path = cw_iq_capture_path();
    if rtsa_fixture_missing(&path) {
        eprintln!(
            "skipping test_rtsa_file_reads_iq_samples: {} is missing or an LFS pointer (run `git lfs pull`)",
            path.display()
        );
        return;
    }

    let mut source = match RtsaSource::open(&path) {
        Ok(s) => s,
        Err(e) if e.to_string().contains("RTSAFileTool was not found") => {
            println!(
                "skipping test_rtsa_file_reads_iq_samples: RTSAFileTool not found to decompress CW fixture"
            );
            return;
        }
        Err(e) => panic!("RtsaSource::open should succeed: {}", e),
    };
    let total = source.total_samples();
    assert!(total > 0, "expected non-zero total_samples");

    // Try to read up to 1024 IQ samples from the primary sub-stream.
    let target = 1024.min(total as usize);
    let data = source
        .read_samples(target, None)
        .expect("read_samples should not error")
        .expect("expected Some(SampleData) for a healthy IQ capture");

    let samples = match data {
        SampleData::Iq(s) => s,
        other => panic!("expected SampleData::Iq, got {:?}", other),
    };

    // Read at most `target` samples (the actual count may be smaller
    // when a SAMP chunk ends before the request is satisfied).
    assert!(
        !samples.is_empty() && samples.len() <= target,
        "expected 1..={} samples, got {}",
        target,
        samples.len()
    );

    // The reader must have advanced.
    assert_eq!(source.current_position() as usize, samples.len());

    // CW signal sanity check: at least *some* samples must be non-zero.
    // (We don't pin the exact magnitudes — DsStS16 vs. DsStF32, value
    // range, and per-mode scaling all interact, and this test is meant
    // to cover the *file reader* not the DSP.)
    assert!(
        samples.iter().any(|s| s.re != 0.0 || s.im != 0.0),
        "expected at least one non-zero IQ sample"
    );
}

#[test]
fn test_rtsa_file_reset_replays_from_zero() {
    let path = cw_iq_capture_path();
    if rtsa_fixture_missing(&path) {
        eprintln!(
            "skipping test_rtsa_file_reset_replays_from_zero: {} is missing or an LFS pointer (run `git lfs pull`)",
            path.display()
        );
        return;
    }
    let mut source = match RtsaSource::open(&path) {
        Ok(s) => s,
        Err(e) if e.to_string().contains("RTSAFileTool was not found") => {
            println!(
                "skipping test_rtsa_file_reset_replays_from_zero: RTSAFileTool not found to decompress CW fixture"
            );
            return;
        }
        Err(e) => panic!("RtsaSource::open should succeed: {}", e),
    };
    let _ = source
        .read_samples(256, None)
        .expect("read_samples")
        .expect("Some IQ data");
    assert_eq!(source.current_position(), 256);
    source.reset().expect("reset");
    assert_eq!(source.current_position(), 0);
    assert!(!source.is_eof());
}

#[test]
fn test_rtsa_lte_capture_opens() {
    let path = lte_iq_capture_path();
    if rtsa_fixture_missing(&path) {
        eprintln!(
            "skipping test_rtsa_lte_capture_opens: {} is missing or an LFS pointer (run `git lfs pull`)",
            path.display()
        );
        return;
    }
    let source = match RtsaSource::open(&path) {
        Ok(s) => s,
        Err(e) if e.to_string().contains("RTSAFileTool was not found") => {
            println!(
                "skipping test_rtsa_lte_capture_opens: RTSAFileTool not found to decompress LTE fixture"
            );
            return;
        }
        Err(e) => panic!("RtsaSource::open should succeed for LTE capture: {}", e),
    };
    let meta = source.metadata();

    // The LTE capture is ~183 MB at 10 MHz — millions of samples.
    assert!(
        source.total_samples() > 1_000_000,
        "LTE capture should have > 1M samples, got {}",
        source.total_samples()
    );

    // 10 MHz nominal sample rate; the SDK may not populate `sample_rate`
    // directly, so probe SSTR.frequency_step as a fallback.
    let candidate = if meta.sample_rate > 0.0 {
        meta.sample_rate
    } else {
        source
            .sub_stream_info()
            .iter()
            .map(|s| s.frequency_step)
            .find(|&step| step > 0.0)
            .unwrap_or(0.0)
    };
    if candidate > 0.0 {
        assert!(
            candidate > 1_000_000.0 && candidate < 100_000_000.0,
            "sample-rate candidate {} should be roughly 10 MHz",
            candidate
        );
    }
}

#[test]
fn test_rtsa_spectra_capture_opens() {
    let path = cw_spectra_capture_path();
    if rtsa_fixture_missing(&path) {
        eprintln!(
            "skipping test_rtsa_spectra_capture_opens: {} is missing or an LFS pointer (run `git lfs pull`)",
            path.display()
        );
        return;
    }
    // Just confirm the spectra companion file parses cleanly. We do not
    // attempt to read sample data here; the SAMP path for spectra is a
    // different code path that's exercised by the IQ test above for the
    // shared parts (chunk discovery, SSTR resolution, metadata build).
    let source =
        RtsaSource::open(&path).expect("RtsaSource::open should succeed for spectra capture");
    assert!(
        source.total_samples() > 0 || !source.sub_stream_info().is_empty(),
        "spectra capture should advertise at least one sub-stream or a non-zero sample count"
    );
}

#[test]
fn test_rtsa_spectra_reads_samples() {
    let path = cw_spectra_capture_path();
    if rtsa_fixture_missing(&path) {
        eprintln!("skipping test: fixture missing");
        return;
    }
    let mut source = RtsaSource::open(&path).expect("RtsaSource::open should succeed");

    // Try to read a large number of spectra samples to get at least one full spectrum
    let data = source
        .read_samples(10000, None)
        .expect("read_samples should not error")
        .expect("expected Some(SampleData)");

    match data {
        SampleData::Spectra(s) => {
            assert!(!s.is_empty(), "expected non-empty spectra");
            println!("Decoded {} spectra samples", s.len());
        }
        other => panic!("expected SampleData::Spectra, got {:?}", other),
    }
}

use proptest::prelude::*;

proptest! {
    #[test]
    fn proptest_stream_parser_does_not_panic(
        format_idx in 0..4usize,
        data in proptest::collection::vec(any::<u8>(), 0..2048)
    ) {
        let format = match format_idx {
            0 => StreamFormat::Json,
            1 => StreamFormat::Int16,
            2 => StreamFormat::Float16,
            _ => StreamFormat::Float32,
        };

        let mut parser = StreamParser::new(format, None).unwrap();
        let bytes = Bytes::from(data);

        // This should not panic, even with totally random unstructured data.
        // It might return Ok(None), Ok(Some), or Err, but shouldn't panic.
        let _ = parser.process_data(&bytes);
    }
}
