use sdr_aaronia_rs::http_streaming::StreamFormat;
use sdr_aaronia_rs::sdr_source::{DwellAdvice, SdrSource, SourceConfig};
use sdr_aaronia_rs::sdr_source_impl::{AaroniaBackend, AaroniaSdrSource};

use std::sync::Arc;
use std::time::{Duration, Instant};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct MockAdvice;
impl DwellAdvice for MockAdvice {
    fn latest_signal_at(&self, _freq_key: u64) -> Option<Instant> {
        None
    }
}

/// A `DwellAdvice` that reports a fixed live-view `channel_override` — the
/// same shape `orecchiette`'s `AppState` presents while a `/video` viewer
/// is connected.
struct OverrideAdvice(f64);
impl DwellAdvice for OverrideAdvice {
    fn latest_signal_at(&self, _freq_key: u64) -> Option<Instant> {
        None
    }
    fn channel_override(&self) -> Option<f64> {
        Some(self.0)
    }
}

/// Poll the mock server until a received request satisfies `pred`, or
/// give up after `deadline`. Returns whether a match was seen.
///
/// The fixed-sleep alternative ("start the source, sleep N ms, assert
/// the request arrived") is load-sensitive: under a fully parallel
/// `cargo test` run the pump thread can be starved past any "long
/// enough" constant, which made these tests flake while passing in
/// isolation. Polling keeps the fast path fast (the first check
/// usually succeeds) and gives the slow path a real deadline.
async fn wait_for_request(
    server: &MockServer,
    deadline: Duration,
    pred: impl Fn(&wiremock::Request) -> bool,
) -> bool {
    let start = Instant::now();
    loop {
        if let Some(requests) = server.received_requests().await
            && requests.iter().any(&pred)
        {
            return true;
        }
        if start.elapsed() > deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// `wait_for_request` predicate: a `/control` PUT whose JSON body retunes
/// `frequencyCenter` to `freq` (within 1 Hz).
fn control_put_tuning_to(freq: f64) -> impl Fn(&wiremock::Request) -> bool {
    move |r: &wiremock::Request| {
        r.url.path() == "/control"
            && r.body_json::<serde_json::Value>()
                .ok()
                .and_then(|v| v.get("frequencyCenter").and_then(|f| f.as_f64()))
                .is_some_and(|f| (f - freq).abs() < 1.0)
    }
}

#[test]
fn test_aaronia_sdr_source_creation() {
    let backend = AaroniaBackend::Http("http://example.com".to_string());

    let source = AaroniaSdrSource {
        backend,
        center_frequency_hz: 1e9,
        reference_level_dbm: 0.0,
        block_size: 1024,
        stream_format: Some(StreamFormat::Float32),
    };

    assert_eq!(source.center_frequency_hz, 1e9);
    assert_eq!(source.stream_format, Some(StreamFormat::Float32));
    assert_eq!(source.reference_level_dbm, 0.0);
}

#[tokio::test]
async fn test_sdr_source_start_single_channel() {
    let mock_server = MockServer::start().await;

    let server_info_json = serde_json::json!({
        "name": "Spectran V6",
        "uuid": "1234-5678-9012",
        "title": "Remote Spectran",
        "port": 54664,
        "mission": "Surveillance"
    });

    Mock::given(method("GET"))
        .and(path("/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&server_info_json))
        .mount(&mock_server)
        .await;

    Mock::given(method("PUT"))
        .and(path("/control"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    Mock::given(method("GET"))
        .and(path("/stream"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0; 100]))
        .mount(&mock_server)
        .await;

    let backend = AaroniaBackend::Http(mock_server.uri());

    let source = AaroniaSdrSource {
        backend,
        center_frequency_hz: 1e9,
        reference_level_dbm: 0.0,
        block_size: 1024,
        stream_format: Some(StreamFormat::Float32),
    };

    let config = SourceConfig {
        sample_rate_hz: 20e6,
        channels_hz: vec![],
        dwell_min: Duration::from_millis(100),
        dwell_max: Duration::from_millis(200),
        dwell_extension: Duration::ZERO,
    };

    let handle = Box::new(source)
        .start(config, Arc::new(MockAdvice))
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    (handle.stop)();
}

#[tokio::test]
async fn test_sdr_source_start_hopping() {
    let mock_server = MockServer::start().await;

    let server_info_json = serde_json::json!({
        "name": "Spectran V6",
        "uuid": "1234-5678-9012",
        "title": "Remote Spectran",
        "port": 54664,
        "mission": "Surveillance"
    });

    Mock::given(method("GET"))
        .and(path("/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&server_info_json))
        .mount(&mock_server)
        .await;

    Mock::given(method("PUT"))
        .and(path("/control"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    Mock::given(method("GET"))
        .and(path("/stream"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0; 100]))
        .mount(&mock_server)
        .await;

    let backend = AaroniaBackend::Http(mock_server.uri());

    let source = AaroniaSdrSource {
        backend,
        center_frequency_hz: 1e9,
        reference_level_dbm: 0.0,
        block_size: 1024,
        stream_format: Some(StreamFormat::Float32),
    };

    let config = SourceConfig {
        sample_rate_hz: 20e6,
        channels_hz: vec![1e9, 2e9],
        dwell_min: Duration::from_millis(50),
        dwell_max: Duration::from_millis(100),
        dwell_extension: Duration::ZERO,
    };

    let handle = Box::new(source)
        .start(config, Arc::new(MockAdvice))
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    (handle.stop)();
}

/// Regression guard for the removed Remote Config license probe: hop mode
/// must retune purely through `/control`'s `configure_capture` and never
/// touch `/remoteconfig` — not even to check license availability first.
/// `/remoteconfig` is deliberately left unmocked; wiremock 404s anything
/// that hits it, and we assert no request ever did.
#[tokio::test]
async fn test_hop_mode_never_touches_remoteconfig() {
    let mock_server = MockServer::start().await;

    let server_info_json = serde_json::json!({
        "name": "Spectran V6", "uuid": "1234-5678-9012",
        "title": "Remote Spectran", "port": 54664, "mission": "Surveillance"
    });
    Mock::given(method("GET"))
        .and(path("/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&server_info_json))
        .mount(&mock_server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/control"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/stream"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0; 100]))
        .mount(&mock_server)
        .await;

    let source = AaroniaSdrSource {
        backend: AaroniaBackend::Http(mock_server.uri()),
        center_frequency_hz: 1e9,
        reference_level_dbm: 0.0,
        block_size: 1024,
        stream_format: Some(StreamFormat::Float32),
    };
    let config = SourceConfig {
        sample_rate_hz: 20e6,
        channels_hz: vec![1e9, 2e9],
        dwell_min: Duration::from_millis(20),
        dwell_max: Duration::from_millis(40),
        dwell_extension: Duration::ZERO,
    };

    let handle = Box::new(source)
        .start(config, Arc::new(MockAdvice))
        .unwrap();
    // Wait until hopping actually exercises `/control` (dwell_max=40ms),
    // not just `/info`/`/stream`.
    let saw_control = wait_for_request(&mock_server, Duration::from_secs(5), |r| {
        r.url.path() == "/control"
    })
    .await;
    (handle.stop)();

    assert!(
        saw_control,
        "expected at least one /control PUT from hopping"
    );
    let requests = mock_server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|r| !r.url.path().contains("remoteconfig")),
        "hop mode must never touch /remoteconfig: {:?}",
        requests.iter().map(|r| r.url.path()).collect::<Vec<_>>()
    );
}

/// The live-view override retunes via the same `/control` `configure_capture`
/// path (never `/remoteconfig`), independent of hop mode.
#[tokio::test]
async fn test_single_channel_honors_channel_override_via_control() {
    let mock_server = MockServer::start().await;

    let server_info_json = serde_json::json!({
        "name": "Spectran V6", "uuid": "1234-5678-9012",
        "title": "Remote Spectran", "port": 54664, "mission": "Surveillance"
    });
    Mock::given(method("GET"))
        .and(path("/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&server_info_json))
        .mount(&mock_server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/control"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/stream"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0; 100]))
        .mount(&mock_server)
        .await;

    let override_freq = 5.8e9;
    let source = AaroniaSdrSource {
        backend: AaroniaBackend::Http(mock_server.uri()),
        center_frequency_hz: 1e9, // base frequency, distinct from the override
        reference_level_dbm: 0.0,
        block_size: 1024,
        stream_format: Some(StreamFormat::Float32),
    };
    let config = SourceConfig {
        sample_rate_hz: 20e6,
        channels_hz: vec![], // non-hopping: exercises single_channel_pump
        dwell_min: Duration::from_millis(100),
        dwell_max: Duration::from_millis(200),
        dwell_extension: Duration::ZERO,
    };

    let handle = Box::new(source)
        .start(config, Arc::new(OverrideAdvice(override_freq)))
        .unwrap();
    // The first /control PUT is the builder's initial capture setup at
    // `center_frequency_hz`, so wait specifically for the PUT that
    // carries the override frequency.
    let retuned_to_override = wait_for_request(
        &mock_server,
        Duration::from_secs(5),
        control_put_tuning_to(override_freq),
    )
    .await;
    (handle.stop)();

    assert!(
        retuned_to_override,
        "no /control PUT carried the override frequency {override_freq}"
    );
    let requests = mock_server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|r| !r.url.path().contains("remoteconfig"))
    );
}

/// The override also parks a hop-configured source (not just a non-hopping
/// one) on the requested channel, bypassing the hop list entirely.
#[tokio::test]
async fn test_hop_mode_honors_channel_override_via_control() {
    let mock_server = MockServer::start().await;

    let server_info_json = serde_json::json!({
        "name": "Spectran V6", "uuid": "1234-5678-9012",
        "title": "Remote Spectran", "port": 54664, "mission": "Surveillance"
    });
    Mock::given(method("GET"))
        .and(path("/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&server_info_json))
        .mount(&mock_server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/control"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/stream"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0; 100]))
        .mount(&mock_server)
        .await;

    // A channel that is *not* in the hop list — proves the override wins
    // over the hop list rather than just happening to match a hop target.
    let override_freq = 5.8e9;
    let source = AaroniaSdrSource {
        backend: AaroniaBackend::Http(mock_server.uri()),
        center_frequency_hz: 1e9,
        reference_level_dbm: 0.0,
        block_size: 1024,
        stream_format: Some(StreamFormat::Float32),
    };
    let config = SourceConfig {
        sample_rate_hz: 20e6,
        channels_hz: vec![1e9, 2e9],
        dwell_min: Duration::from_millis(20),
        dwell_max: Duration::from_millis(40),
        dwell_extension: Duration::ZERO,
    };

    let handle = Box::new(source)
        .start(config, Arc::new(OverrideAdvice(override_freq)))
        .unwrap();
    // Wait for the override to be requested; the window this spans covers
    // several would-be hop dwells (dwell_max=40ms), so if the override
    // weren't honored we'd see /control PUTs for 1e9/2e9 instead.
    let saw_override = wait_for_request(
        &mock_server,
        Duration::from_secs(5),
        control_put_tuning_to(override_freq),
    )
    .await;
    (handle.stop)();

    // The very first PUT is the builder's initial capture setup (always at
    // `center_frequency_hz`, before `hop_pump` runs), so we can't require
    // *every* PUT to be the override. What proves the hop list was bypassed
    // is: the override frequency was actually requested, and the second hop
    // channel (2e9, never legitimately reachable except by real hopping)
    // never was.
    assert!(
        saw_override,
        "override frequency {override_freq} never appeared in a /control PUT"
    );
    let requests = mock_server.received_requests().await.unwrap();
    let freqs: Vec<f64> = requests
        .iter()
        .filter(|r| r.url.path() == "/control")
        .filter_map(|r| r.body_json::<serde_json::Value>().ok())
        .filter_map(|v| v.get("frequencyCenter").and_then(|f| f.as_f64()))
        .collect();
    assert!(
        freqs.iter().all(|f| (f - 2e9).abs() > 1.0),
        "hop channel 2e9 should never be reached while overridden: {freqs:?}"
    );
}
