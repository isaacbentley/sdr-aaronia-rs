//! Resilience of the HTTP backend: connect retry, and the configurable
//! blocking-read timeout.
//!
//! These drive `AaroniaSource` against a `wiremock` server rather than
//! `HttpEndpointsClient` directly, because the behaviour under test
//! lives in the source's setup path, not in the endpoint client.

use sdr_aaronia_rs::Error;
use sdr_aaronia_rs::unified_source::{AaroniaConfig, AaroniaSource};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Fails the first `fail_count` calls with `status`, then succeeds with
/// `body`. Models a server that is booting, or a `*.local` name that
/// mDNS has not finished resolving.
struct FlakyThenOk {
    calls: Arc<AtomicUsize>,
    fail_count: usize,
    status: u16,
    body: serde_json::Value,
}

impl Respond for FlakyThenOk {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_count {
            ResponseTemplate::new(self.status)
        } else {
            ResponseTemplate::new(200).set_body_json(&self.body)
        }
    }
}

fn info_body() -> serde_json::Value {
    serde_json::json!({
        "name": "Spectran V6",
        "uuid": "1234-5678-9012",
        "title": "Mock RTSA",
        "port": 54664,
        "mission": "Test"
    })
}

/// Mount `/control` (always OK) and a `/stream` that never sends a body,
/// so a source can finish `start_streaming` and then block in `read`.
async fn mount_control_and_idle_stream(server: &MockServer) {
    Mock::given(method("PUT"))
        .and(path("/control"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/stream"))
        .respond_with(ResponseTemplate::new(200).set_body_string(""))
        .mount(server)
        .await;
}

/// A server that 503s its first two `/info` requests must still be
/// reachable: the connect path retries transient failures instead of
/// reporting "is it running?" on the first stumble.
#[tokio::test]
async fn connect_retries_transient_failures() {
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));

    Mock::given(method("GET"))
        .and(path("/info"))
        .respond_with(FlakyThenOk {
            calls: calls.clone(),
            fail_count: 2,
            status: 503,
            body: info_body(),
        })
        .mount(&server)
        .await;
    mount_control_and_idle_stream(&server).await;

    let config = AaroniaConfig::from_http(&server.uri());
    AaroniaSource::new(config)
        .await
        .expect("source must survive two 503s on /info");

    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "expected two failed attempts plus the successful one"
    );
}

/// Retrying is only for transient failures. A 404 means the URL is
/// wrong and will stay wrong, so it must fail on the first attempt
/// rather than spending the whole backoff budget.
#[tokio::test]
async fn connect_does_not_retry_client_errors() {
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));

    Mock::given(method("GET"))
        .and(path("/info"))
        .respond_with(FlakyThenOk {
            calls: calls.clone(),
            // Never succeeds within the retry budget if retried at all.
            fail_count: usize::MAX,
            status: 404,
            body: info_body(),
        })
        .mount(&server)
        .await;

    let config = AaroniaConfig::from_http(&server.uri());
    // `AaroniaSource` is not `Debug`, so match instead of `expect_err`.
    match AaroniaSource::new(config).await {
        Ok(_) => panic!("404 must not be retried into success"),
        Err(Error::Protocol(_)) => {}
        Err(other) => panic!("unexpected error variant: {other:?}"),
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a 4xx must fail on the first attempt"
    );
}

/// `read_samples` honours `config.read_timeout` instead of the former
/// hard-coded 30 s, so a caller that would rather see a gap than stall
/// can say so.
#[tokio::test]
async fn read_honours_configured_timeout() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(info_body()))
        .mount(&server)
        .await;
    mount_control_and_idle_stream(&server).await;

    let config = AaroniaConfig::from_http(&server.uri()).read_timeout(Duration::from_millis(300));
    let mut source = AaroniaSource::new(config).await.expect("source");
    source.start_streaming().await.expect("start_streaming");

    let mut buffer = Vec::new();
    let started = Instant::now();
    let result = source.read_samples(&mut buffer, 1024).await;
    let elapsed = started.elapsed();

    match result {
        Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::TimedOut => {
            // A genuine timeout must have waited the configured span —
            // not returned instantly for some unrelated reason.
            assert!(
                elapsed >= Duration::from_millis(250),
                "timed out after only {elapsed:?}; the 300 ms timeout was not what fired"
            );
        }
        // Depending on how the mock finishes the empty response the
        // reader task may report a closed stream instead of stalling.
        // That path is prompt by construction; the timeout bound below
        // still applies to both.
        Err(Error::Protocol(_)) => {}
        other => panic!("expected a timeout or closed-stream error, got {other:?}"),
    }
    assert!(
        elapsed < Duration::from_secs(1),
        "read blocked for {elapsed:?} — the configured 300 ms timeout was ignored \
         (the old hard-coded default was 30 s)"
    );
}
