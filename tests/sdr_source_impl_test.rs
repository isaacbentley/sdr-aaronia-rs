use sdr_aaronia_rs::sdr_source_impl::{AaroniaBackend, AaroniaSdrSource};
use sdr_aaronia_rs::sdr_source::{SdrSource, SourceConfig, DwellAdvice};
use sdr_aaronia_rs::http_streaming::StreamFormat;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use std::time::{Duration, Instant};
use std::sync::Arc;

struct MockAdvice;
impl DwellAdvice for MockAdvice {
    fn latest_signal_at(&self, _freq_key: u64) -> Option<Instant> {
        None
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

    let handle = Box::new(source).start(config, Arc::new(MockAdvice)).unwrap();

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

    let handle = Box::new(source).start(config, Arc::new(MockAdvice)).unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    (handle.stop)();
}
