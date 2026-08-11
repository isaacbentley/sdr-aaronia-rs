//! Live smoke tests against a running RTSA-Suite PRO HTTP server block.
//!
//! All tests are `#[ignore]`d: they need real hardware/software at
//! `AARONIA_LIVE_URL` (default `http://atc.local:54664`). Run with:
//!
//! ```sh
//! cargo test --test live_smoke -- --ignored --nocapture
//! ```
//!
//! These exercise the paths that unit tests can only mock: the
//! control-plane client, the streaming parser against real wire data
//! (both float32 and int16), and the unified-source pipeline.

use futures::stream::StreamExt;
use sdr_aaronia_rs::http_endpoints::{
    AuthMethod, HttpEndpointsClient, InputProcessingType, StreamParams, TxSampleRequest,
};
use sdr_aaronia_rs::http_streaming::{DropDetector, PayloadType, StreamFormat};
use std::time::{Duration, Instant};

fn live_url() -> String {
    std::env::var("AARONIA_LIVE_URL").unwrap_or_else(|_| "http://atc.local:54664".to_string())
}

fn client() -> HttpEndpointsClient {
    HttpEndpointsClient::new(live_url(), AuthMethod::None).expect("client construction")
}

#[tokio::test]
#[ignore = "requires live RTSA-Suite PRO at AARONIA_LIVE_URL / atc.local:54664"]
async fn live_control_plane() {
    let c = client();

    c.test_connection().await.expect("connection test");

    let info = c.get_info().await.expect("/info");
    println!(
        "server: name={} title={} port={} mission={}",
        info.name, info.title, info.port, info.mission
    );
    assert!(!info.uuid.is_empty(), "server must report a uuid");
    assert_eq!(info.port, 54664);

    let inputs = c.get_inputs().await.expect("/inputs");
    println!("inputs: {:?}", inputs);

    let health = c.get_health_status().await.expect("/healthstatus");
    println!(
        "healthstatus parsed OK: {:?}",
        std::mem::discriminant(&health)
    );

    // Read-only license check must not touch device state.
    let status = c.detect_remote_config_license().await;
    println!("remote-config (read-only) status: {:?}", status);
}

/// Find an input on the server whose current payload matches `want`.
/// Missions differ in what they wire into the HTTP server (IQ, spectra,
/// or both), so tests discover instead of assuming. Returns the input
/// name, or `None` if no input currently carries that payload type.
async fn input_with_payload(c: &HttpEndpointsClient, want: &PayloadType) -> Option<String> {
    let inputs = c.get_inputs().await.expect("/inputs");
    for name in inputs {
        if c.get_sample(Some(&name))
            .await
            .is_ok_and(|sample| sample.payload == *want)
        {
            return Some(name);
        }
    }
    None
}

/// Aggregate stats from pumping a stream for a fixed wall-clock window.
struct PumpStats {
    packets: u64,
    samples: u64,
    rate: f64,
    center: f64,
    errors: u64,
    /// Largest |re| component seen — catches byte-shift / scale bugs
    /// that produce absurd amplitudes while still "parsing" cleanly.
    max_abs: f32,
}

/// Stream `secs` seconds of packets in `format` from the default input.
async fn pump_stream(format: StreamFormat, secs: u64) -> PumpStats {
    let params = HttpEndpointsClient::stream_params().format(format).build();
    pump_stream_with_params(params, secs).await
}

/// As [`pump_stream`], but with caller-supplied stream parameters.
async fn pump_stream_with_params(params: StreamParams, secs: u64) -> PumpStats {
    let c = client();
    let mut stream = c.start_stream(params).await.expect("start_stream");

    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut stats = PumpStats {
        packets: 0,
        samples: 0,
        rate: 0.0,
        center: 0.0,
        errors: 0,
        max_abs: 0.0,
    };

    while Instant::now() < deadline {
        // Generous per-packet timeout: live networks have latency spikes,
        // and a flaky live test helps nobody. A healthy stream delivers
        // hundreds of packets per second.
        let next = tokio::time::timeout(Duration::from_secs(15), stream.next()).await;
        match next {
            Ok(Some(Ok(pkt))) => {
                stats.packets += 1;
                stats.samples += pkt.samples.len() as u64;
                stats.rate = pkt.sdr_config.sample_rate;
                stats.center = pkt.sdr_config.center_frequency;
                for s in &pkt.samples {
                    stats.max_abs = stats.max_abs.max(s.re.abs()).max(s.im.abs());
                }
            }
            Ok(Some(Err(e))) => {
                stats.errors += 1;
                eprintln!("stream item error: {e:#}");
            }
            Ok(None) => panic!("stream ended prematurely"),
            Err(_) => panic!("no packet within 15 s — is the mission streaming?"),
        }
    }
    stats
}

/// Pump an IQ stream in `format` from a discovered IQ input; skips (with
/// a message) when the current mission exposes no IQ input.
async fn iq_stream_case(format: StreamFormat, secs: u64, label: &str) {
    let c = client();
    let Some(input) = input_with_payload(&c, &PayloadType::Iq).await else {
        println!("SKIP {label}: no IQ input on this mission");
        return;
    };
    let params = HttpEndpointsClient::stream_params()
        .format(format)
        .input(input)
        .build();
    let s = pump_stream_with_params(params, secs).await;
    println!(
        "{label}: {} packets, {} samples, rate={:.0} Hz, center={:.3} MHz, max|v|={:.4}, {} errors",
        s.packets,
        s.samples,
        s.rate,
        s.center / 1e6,
        s.max_abs,
        s.errors
    );
    assert!(s.packets > 0, "must decode at least one packet");
    assert!(s.samples > 0, "must decode samples");
    assert!(s.rate > 0.0 && s.center > 0.0);
    assert_eq!(s.errors, 0, "no parse errors expected on a healthy stream");
    // IQ components are volts at the frontend — a handful at most.
    // Byte-shifted floats decode as ~1e14 and mis-scaled int16 as ~1e8,
    // so this bound is the live regression check for both bugs.
    assert!(
        s.max_abs.is_finite() && s.max_abs < 100.0,
        "IQ amplitudes implausible (byte-shift or scale bug?): max |v| = {}",
        s.max_abs
    );
}

#[tokio::test]
#[ignore = "requires live RTSA-Suite PRO at AARONIA_LIVE_URL / atc.local:54664"]
async fn live_stream_float32() {
    iq_stream_case(StreamFormat::Float32, 5, "float32").await;
}

#[tokio::test]
#[ignore = "requires live RTSA-Suite PRO at AARONIA_LIVE_URL / atc.local:54664"]
async fn live_stream_int16() {
    iq_stream_case(StreamFormat::Int16, 5, "int16").await;
}

/// The 120 s client-timeout regression: before the fix, any stream
/// through `HttpEndpointsClient` died at exactly 120 s. Streaming for
/// >130 s proves the fix. Slow — run explicitly when needed.
#[tokio::test]
#[ignore = "slow (~135 s); requires live RTSA-Suite PRO"]
async fn live_stream_survives_past_120s() {
    let s = pump_stream(StreamFormat::Int16, 135).await;
    println!(
        "long-run: {} packets, {} samples over 135 s",
        s.packets, s.samples
    );
    assert!(s.packets > 0);
}

/// Stream real spectra packets and validate the frame layout end-to-end:
/// per-packet bin count must equal `samples × sampleSize × depth` (the
/// live-verified layout), and every bin must be a plausible dBm value.
/// Skips when the mission has no spectra input.
async fn spectra_stream_case(format: StreamFormat, label: &str) {
    let c = client();
    let Some(input) = input_with_payload(&c, &PayloadType::Spectra).await else {
        println!("SKIP {label}: no spectra input on this mission");
        return;
    };
    let params = HttpEndpointsClient::stream_params()
        .format(format)
        .input(input)
        .build();
    let mut stream = c.start_stream(params).await.expect("start_stream");

    let mut checked = 0u32;
    while checked < 10 {
        let pkt = tokio::time::timeout(Duration::from_secs(15), stream.next())
            .await
            .expect("no spectra packet within 15 s")
            .expect("stream ended prematurely")
            .expect("stream error");

        assert_eq!(pkt.metadata.payload, PayloadType::Spectra);
        let depth = pkt.metadata.sample_depth.unwrap_or(1).max(1) as u64;
        let expected_bins = pkt.metadata.samples * pkt.metadata.sample_size as u64 * depth;
        assert_eq!(
            pkt.samples.len() as u64,
            expected_bins,
            "bin count must be frames × bins × depth"
        );
        // Bins are dBm power levels. The packet advertises its own range;
        // allow generous slack for AGC movement between header and data.
        let (lo, hi) = (
            pkt.metadata.min_power as f32 - 60.0,
            pkt.metadata.max_power as f32 + 60.0,
        );
        for bin in &pkt.samples {
            assert!(
                bin.re.is_finite() && bin.re >= lo && bin.re <= hi,
                "bin {} dBm outside plausible range {lo}..{hi} \
                 (separator/scale/size bug?)",
                bin.re
            );
            assert_eq!(bin.im, 0.0, "spectra bins are real-valued");
        }
        checked += 1;
    }
    println!("{label}: {checked} spectra packets validated (payload, frame layout, dBm ranges)");
}

#[tokio::test]
#[ignore = "requires live RTSA-Suite PRO with a spectra input (e.g. IQ Power Spectrum block)"]
async fn live_stream_spectra_float32() {
    spectra_stream_case(StreamFormat::Float32, "spectra/float32").await;
}

/// Int16 spectra is the strongest end-to-end check in the suite: it
/// exercises the two-byte separator, the frame-count size formula, AND
/// the divide-by-scale semantics at once — any of the three being wrong
/// puts bins far outside the advertised dBm range.
#[tokio::test]
#[ignore = "requires live RTSA-Suite PRO with a spectra input (e.g. IQ Power Spectrum block)"]
async fn live_stream_spectra_int16() {
    spectra_stream_case(StreamFormat::Int16, "spectra/int16").await;
}

/// `/sample` returns a single JSON document whose `samples` field is the
/// *array* form — this exercises the dual-form `samples` deserializer
/// against real wire data. `/samples` returns a JSON array of documents.
#[tokio::test]
#[ignore = "requires live RTSA-Suite PRO at AARONIA_LIVE_URL / atc.local:54664"]
async fn live_sample_endpoints() {
    let c = client();

    let sample = c.get_sample(None).await.expect("/sample");
    println!(
        "/sample: payload={:?} samples={} rate={:?} range={:.3}..{:.3} MHz",
        sample.payload,
        sample.samples,
        sample.sample_frequency,
        sample.start_frequency / 1e6,
        sample.end_frequency / 1e6
    );
    assert!(sample.samples > 0, "array-form samples field must count");
    assert!(sample.end_frequency > sample.start_frequency);

    // The explicit `input=` also exercises the percent-encoded query path.
    let inputs = c.get_inputs().await.expect("/inputs");
    let first = inputs.first().expect("at least one input");
    let batch = c.get_samples(Some(2), Some(first)).await.expect("/samples");
    println!("/samples?limit=2&input={first}: {} documents", batch.len());
    assert_eq!(batch.len(), 2);
    for m in &batch {
        assert!(m.samples > 0);
    }
}

/// Pure-JSON stream format: samples arrive inside the JSON documents.
/// Exercises `parse_json_samples` + the pair-count reconciliation on
/// real data (the offline tests only cover synthetic documents).
#[tokio::test]
#[ignore = "requires live RTSA-Suite PRO at AARONIA_LIVE_URL / atc.local:54664"]
async fn live_stream_json_format() {
    let s = pump_stream(StreamFormat::Json, 3).await;
    println!(
        "json: {} packets, {} samples, rate={:.0} Hz, center={:.3} MHz, {} errors",
        s.packets,
        s.samples,
        s.rate,
        s.center / 1e6,
        s.errors
    );
    assert!(s.packets > 0 && s.samples > 0);
    assert!(s.rate > 0.0 && s.center > 0.0);
    assert_eq!(s.errors, 0);
}

#[tokio::test]
#[ignore = "requires live RTSA-Suite PRO at AARONIA_LIVE_URL / atc.local:54664"]
async fn live_stream_float16() {
    let s = pump_stream(StreamFormat::Float16, 3).await;
    println!(
        "float16: {} packets, {} samples, rate={:.0} Hz, center={:.3} MHz, {} errors",
        s.packets,
        s.samples,
        s.rate,
        s.center / 1e6,
        s.errors
    );
    assert!(s.packets > 0 && s.samples > 0);
    assert!(s.rate > 0.0 && s.center > 0.0);
    assert_eq!(s.errors, 0);
}

/// `?limit=N` must deliver exactly N packets and then end the stream
/// cleanly (server closes the connection). Exercises end-of-stream
/// handling, which the timed pumps never reach.
#[tokio::test]
#[ignore = "requires live RTSA-Suite PRO at AARONIA_LIVE_URL / atc.local:54664"]
async fn live_stream_limit_ends_stream() {
    let c = client();
    let params = HttpEndpointsClient::stream_params()
        .format(StreamFormat::Int16)
        .limit(3)
        .build();
    let mut stream = c.start_stream(params).await.expect("start_stream");

    let mut packets = 0u64;
    loop {
        match tokio::time::timeout(Duration::from_secs(15), stream.next()).await {
            Ok(Some(Ok(_pkt))) => packets += 1,
            Ok(Some(Err(e))) => panic!("unexpected stream error: {e:#}"),
            Ok(None) => break, // clean end-of-stream
            Err(_) => panic!("stream neither delivered nor ended within 15 s"),
        }
        assert!(packets <= 3, "limit=3 must not deliver more than 3 packets");
    }
    println!("limit=3: got {packets} packets, then clean end-of-stream");
    assert_eq!(packets, 3);
}

/// `rate_reduction` + server-side `scale` must be accepted and still
/// produce decodable packets (exercises the full query-parameter set).
#[tokio::test]
#[ignore = "requires live RTSA-Suite PRO at AARONIA_LIVE_URL / atc.local:54664"]
async fn live_stream_rate_reduction_and_scale() {
    let params = HttpEndpointsClient::stream_params()
        .format(StreamFormat::Int16)
        .rate_reduction(64)
        .scale(16384.0)
        .build();
    let s = pump_stream_with_params(params, 3).await;
    println!(
        "rate_reduction=64: {} packets, {} samples, reported rate={:.0} Hz",
        s.packets, s.samples, s.rate
    );
    assert!(s.packets > 0 && s.samples > 0);
    assert_eq!(s.errors, 0);
}

/// A full-rate IQ device produces ~61.44 Msps of float32 IQ (~490 MB/s)
/// — far more than the network can carry — so the RTSA server *must*
/// drop data (its documented behaviour past an 8 MB TCP backlog).
/// `DropDetector` should observe those gaps in the packet timestamps.
/// Requires an IQ input for the oversubscription argument to hold;
/// low-bandwidth spectra streams may be gap-free.
#[tokio::test]
#[ignore = "requires live RTSA-Suite PRO with an IQ input at AARONIA_LIVE_URL"]
async fn live_drop_detector_flags_oversubscribed_link() {
    let c = client();
    let Some(input) = input_with_payload(&c, &PayloadType::Iq).await else {
        println!("SKIP drop detector: no IQ input on this mission");
        return;
    };
    let params = HttpEndpointsClient::stream_params()
        .format(StreamFormat::Float32)
        .input(input)
        .build();
    let mut stream = c.start_stream(params).await.expect("start_stream");

    let mut detector = DropDetector::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut packets = 0u64;
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(15), stream.next()).await {
            Ok(Some(Ok(pkt))) => {
                packets += 1;
                let _ = detector.observe(&pkt);
            }
            Ok(Some(Err(e))) => panic!("stream error: {e:#}"),
            Ok(None) => break,
            Err(_) => panic!("no packet within 15 s"),
        }
    }
    println!(
        "drop detector: {packets} packets, {} drops, {:.3} s cumulative gap",
        detector.drops(),
        detector.cumulative_gap_seconds()
    );
    assert!(packets > 0);
    assert!(
        detector.drops() > 0,
        "a ~490 MB/s float32 stream over the network must exhibit server-side drops"
    );
}

/// This mission's HTTP server block does not support creating processed
/// inputs (spectra via average/maxhold) — POST /inputs returns 404. The
/// failure must be classifiable via the typed `Error::Http` in the
/// error chain, not just error text. If a future mission *does* support
/// it, the created input name is reported instead.
#[tokio::test]
#[ignore = "requires live RTSA-Suite PRO at AARONIA_LIVE_URL / atc.local:54664"]
async fn live_create_input_failure_is_typed() {
    let c = client();
    match c.create_input("main", InputProcessingType::Average).await {
        Ok(name) => println!("device supports processed inputs; created: {name}"),
        Err(e) => {
            let is_http_err = matches!(e, sdr_aaronia_rs::Error::Http { .. });
            println!("create_input failed as expected: http_err={is_http_err} ({e:#})");
            assert!(
                is_http_err,
                "HTTP failures must carry an Http error in the chain"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires live RTSA-Suite PRO with an IQ input at AARONIA_LIVE_URL"]
async fn live_unified_source() {
    use sdr_aaronia_rs::{AaroniaConfig, AaroniaSource};

    // The unified source consumes IQ; skip when the mission only
    // exposes spectra.
    if input_with_payload(&client(), &PayloadType::Iq)
        .await
        .is_none()
    {
        println!("SKIP unified source: no IQ input on this mission");
        return;
    }

    let config = AaroniaConfig::from_http(&live_url());
    let mut source = AaroniaSource::new(config).await.expect("unified source");
    source.start_streaming().await.expect("start_streaming");

    let mut buffer = Vec::with_capacity(65_536);
    let n = source
        .read_samples(&mut buffer, 65_536)
        .await
        .expect("read_samples");
    println!(
        "unified source: {} samples, info: {}",
        n,
        source.get_source_info()
    );
    assert!(n > 0, "unified HTTP source must yield samples");
    assert_eq!(n, buffer.len());
    assert!(
        buffer.iter().any(|s| s.re != 0.0 || s.im != 0.0),
        "samples should not be all-zero"
    );

    source.stop_streaming().await.expect("stop_streaming");
}

/// Push a single small burst of IQ samples via `POST /sample`. Confirms
/// the HTTP TX path (`HttpEndpointsClient::push_samples`, and the
/// `HttpSink` FutureSDR block that wraps it) is actually accepted by a
/// real RTSA-Suite PRO server — unlike the native `TxStream` path (no TX
/// hardware available locally to verify against), the HTTP endpoint is
/// live-testable regardless of the attached device's capabilities, since
/// `/sample` accepts pushed samples independent of RX/TX support.
#[tokio::test]
#[ignore = "requires live RTSA-Suite PRO at AARONIA_LIVE_URL / atc.local:54664"]
async fn live_tx_push_sample() {
    let c = client();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    // Schedule a few seconds out so the server has time to accept and
    // queue the burst rather than rejecting a start_time in the past.
    let start_time = now + 2.0;
    let sample_rate = 1e6;
    let samples: Vec<f32> = (0..2048)
        .flat_map(|i| {
            let phase = i as f32 * 0.1;
            [phase.cos() * 0.01, phase.sin() * 0.01]
        })
        .collect();
    let num_complex = samples.len() / 2;
    let end_time = start_time + num_complex as f64 / sample_rate;

    let req = TxSampleRequest {
        start_time,
        end_time,
        start_frequency: 299e6,
        end_frequency: 301e6,
        step_frequency: Some(sample_rate),
        min_power: -2.0,
        max_power: 2.0,
        sample_size: 2,
        sample_depth: 1,
        unit: "volt".to_string(),
        payload: "iq".to_string(),
        push: true,
        samples: &samples,
    };

    c.push_samples(&req).await.expect("push_samples");
    println!(
        "pushed {} complex samples via /sample (start_time={:.3}, end_time={:.3})",
        num_complex, start_time, end_time
    );
}

#[test]
#[ignore = "requires live RTSA-Suite PRO at AARONIA_LIVE_URL"]
#[cfg(feature = "seify")]
fn live_stream_seify() {
    use sdr_aaronia_rs::seify_impl::AaroniaSeifyDevice;
    use seify::dev::DynDeviceBackend;
    use seify::{Args, DeviceInfo, RxStreamer};

    let url = live_url();
    let mut args = Args::new();
    args.set("url", url);

    let dev = AaroniaSeifyDevice::from_args(&args).expect("seify open");

    let info = dev.info().expect("info");
    println!("seify info: {:?}", info);

    let rx = dev.rx_device().expect("rx_device");
    let mut streamer = rx.rx_streamer(&[0], Args::new()).expect("rx_streamer");

    streamer.activate_at(None).expect("activate");

    let mut buffer = [num_complex::Complex32::new(0.0, 0.0); 1024];

    // Generous timeout for streaming to start
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut total_read = 0;

    while Instant::now() < deadline && total_read == 0 {
        match streamer.read(&mut [&mut buffer], 1_000_000) {
            Ok(n) if n > 0 => {
                total_read += n;
                break;
            }
            Ok(_) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                if !e.to_string().contains("timeout")
                    && !e.to_string().contains("Resource temporarily unavailable")
                {
                    panic!("seify read error: {}", e);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    println!("seify read {} samples", total_read);
    assert!(total_read > 0, "must read at least one packet");
    assert!(
        buffer[0..total_read]
            .iter()
            .any(|s| s.re != 0.0 || s.im != 0.0),
        "samples should not be all-zero"
    );

    streamer.deactivate_at(None).expect("deactivate");
}

#[test]
#[ignore = "requires live RTSA-Suite PRO at AARONIA_LIVE_URL and SOAPY_SDR_PLUGIN_PATH set"]
fn live_stream_soapy() {
    // This test requires the SoapySDR Aaronia plugin to be built and accessible via SOAPY_SDR_PLUGIN_PATH
    // E.g., SOAPY_SDR_PLUGIN_PATH=$(pwd)/soapy-aaronia/build cargo test ...
    let url = live_url();
    let args = format!("driver=aaronia,url={}", url);

    let dev = match soapysdr::Device::new(args.as_str()) {
        Ok(dev) => dev,
        Err(e) => {
            println!(
                "SKIP soapy test: failed to open soapy device: {} (plugin may not be built/loaded)",
                e
            );
            return;
        }
    };

    let hw = dev.hardware_key().expect("hardware_key");
    println!("soapy opened hardware: {}", hw);
    assert_eq!(hw, "Spectran V6");

    let mut stream = dev
        .rx_stream::<num_complex::Complex<i16>>(&[0])
        .expect("rx_stream");
    stream.activate(None).expect("activate");

    let mut buffer = [num_complex::Complex::<i16>::new(0, 0); 1024];
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut total_read = 0;

    while Instant::now() < deadline && total_read == 0 {
        match stream.read(&mut [&mut buffer], 1_000_000) {
            Ok(n) if n > 0 => {
                total_read += n;
                break;
            }
            Ok(_) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) if e.code == soapysdr::ErrorCode::Timeout => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                panic!("soapy read error: {}", e);
            }
        }
    }

    println!("soapy read {} samples", total_read);
    assert!(total_read > 0, "must read at least one packet");
    assert!(
        buffer[0..total_read].iter().any(|s| s.re != 0 || s.im != 0),
        "samples should not be all-zero"
    );

    stream.deactivate(None).expect("deactivate");
}
