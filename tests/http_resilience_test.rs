//! Resilience of the HTTP backend: connect retry, stream
//! auto-reconnect, and the configurable blocking-read timeout.
//!
//! These drive `AaroniaSource` against a `wiremock` server rather than
//! `HttpEndpointsClient` directly, because the behaviour under test
//! lives in the source's setup and reader-task paths, not in the
//! endpoint client.

use sdr_aaronia_rs::Error;
use sdr_aaronia_rs::http_streaming::StreamFormat;
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

/// One JSON-format IQ packet carrying `count` samples, timestamped so
/// consecutive packets look contiguous to the drop detector.
fn json_packet(index: u64, count: usize) -> String {
    let start = index as f64;
    let samples: Vec<String> = (0..count)
        .flat_map(|_| ["1.5".into(), "-2.5".into()])
        .collect();
    format!(
        r#"{{"startTime":{start},"endTime":{end},"startFrequency":0.0,"endFrequency":1.0,"unit":"volt","payload":"iq","minPower":0,"maxPower":1,"sampleSize":2,"samples":[{s}]}}"#,
        end = start + 1.0,
        s = samples.join(",")
    ) + "\n"
}

/// A finite `/stream` body: a burst of packets, then end-of-body. Every
/// request gets a fresh burst, so a reconnecting reader keeps receiving
/// data while the mock counts how many times the stream was opened.
struct BurstyStream {
    opens: Arc<AtomicUsize>,
    packets_per_open: usize,
    samples_per_packet: usize,
}

impl Respond for BurstyStream {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let open = self.opens.fetch_add(1, Ordering::SeqCst) as u64;
        let body: String = (0..self.packets_per_open)
            .map(|i| {
                json_packet(
                    open * self.packets_per_open as u64 + i as u64,
                    self.samples_per_packet,
                )
            })
            .collect();
        ResponseTemplate::new(200).set_body_string(body)
    }
}

/// Count `/control` PUTs whose body carries the full capture tuple.
async fn full_tuple_control_puts(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == "/control")
        .filter_map(|r| r.body_json::<serde_json::Value>().ok())
        .filter(|v| {
            v.get("frequencyCenter").and_then(|f| f.as_f64()).is_some()
                && v.get("frequencySpan").and_then(|f| f.as_f64()).is_some()
        })
        .count()
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

/// The stream ending mid-session (RTSA restart, network blip) used to
/// end the session permanently. With `auto_reconnect` — the default —
/// the reader reopens the stream, re-applies tuning, and keeps
/// delivering samples across the gap.
#[tokio::test]
async fn stream_reconnects_and_reapplies_tuning() {
    let server = MockServer::start().await;
    let opens = Arc::new(AtomicUsize::new(0));

    Mock::given(method("GET"))
        .and(path("/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(info_body()))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/control"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true
        })))
        .mount(&server)
        .await;
    // Two packets of 4 samples per connection: reading 12 samples has
    // to span at least two connections, so a working read proves the
    // reconnect happened rather than merely being attempted.
    Mock::given(method("GET"))
        .and(path("/stream"))
        .respond_with(BurstyStream {
            opens: opens.clone(),
            packets_per_open: 2,
            samples_per_packet: 4,
        })
        .mount(&server)
        .await;

    let config = AaroniaConfig::from_http(&server.uri())
        .stream_format(StreamFormat::Json)
        .read_timeout(Duration::from_secs(20));
    let mut source = AaroniaSource::new(config).await.expect("source");
    source.start_streaming().await.expect("start_streaming");

    let puts_after_start = full_tuple_control_puts(&server).await;

    let mut buffer = Vec::new();
    source
        .read_samples(&mut buffer, 12)
        .await
        .expect("reads must continue across a reconnect");

    assert_eq!(
        buffer.len(),
        12,
        "expected samples from several connections"
    );
    assert!(
        opens.load(Ordering::SeqCst) >= 2,
        "the stream was only opened once — no reconnect happened"
    );
    assert!(
        full_tuple_control_puts(&server).await > puts_after_start,
        "reconnect must re-apply the tuning, or a restarted server \
         silently streams its mission's frequency instead"
    );
    assert!(
        source.take_overrun(),
        "samples were missed across the gap; the first packet after a \
         reconnect must be flagged as an overrun"
    );
}

/// A server that accepts, sends one packet, and hangs up — repeatedly —
/// must exhaust the reconnect budget rather than reconnecting forever.
/// Resetting the attempt counter on "a packet arrived" would make this
/// loop indefinitely; the counter resets on connection *uptime*.
#[tokio::test]
async fn flapping_server_exhausts_reconnect_budget() {
    let server = MockServer::start().await;
    let opens = Arc::new(AtomicUsize::new(0));

    Mock::given(method("GET"))
        .and(path("/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(info_body()))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/control"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true
        })))
        .mount(&server)
        .await;
    // One tiny packet per connection, forever: always "working", never
    // healthy.
    Mock::given(method("GET"))
        .and(path("/stream"))
        .respond_with(BurstyStream {
            opens: opens.clone(),
            packets_per_open: 1,
            samples_per_packet: 1,
        })
        .mount(&server)
        .await;

    let config = AaroniaConfig::from_http(&server.uri())
        .stream_format(StreamFormat::Json)
        .read_timeout(Duration::from_secs(30));
    let mut source = AaroniaSource::new(config).await.expect("source");
    source.start_streaming().await.expect("start_streaming");

    // Far more samples than the flapping server will ever deliver.
    let mut buffer = Vec::new();
    let started = Instant::now();
    let result = source.read_samples(&mut buffer, 1_000_000).await;
    let elapsed = started.elapsed();

    match result {
        // Whatever the flapping server managed to deliver comes back as
        // a partial read once the stream is finally declared dead; a
        // closed-stream error is equally valid if nothing arrived.
        Ok(n) => assert!(
            n < 1_000_000,
            "the read should not have been satisfied by a flapping server"
        ),
        Err(Error::Protocol(_)) => {}
        other => panic!("expected a partial read or closed-stream error, got {other:?}"),
    }
    // 5 attempts of 0.25+0.5+1+2+4 s ≈ 7.75 s, well inside the 30 s read
    // timeout — proving the *reconnect budget* ended the read, not the
    // timeout, and that retries did not continue indefinitely.
    assert!(
        elapsed < Duration::from_secs(20),
        "took {elapsed:?}; the reconnect budget did not bound the retries"
    );
    let total_opens = opens.load(Ordering::SeqCst);
    assert!(
        (2..=8).contains(&total_opens),
        "expected the bounded retry budget to be spent, saw {total_opens} connections"
    );
}

/// Opting out restores the previous contract exactly: one connection,
/// and a closed stream surfaces as an error rather than reconnecting
/// behind the caller's back.
#[tokio::test]
async fn auto_reconnect_disabled_keeps_fail_fast() {
    let server = MockServer::start().await;
    let opens = Arc::new(AtomicUsize::new(0));

    Mock::given(method("GET"))
        .and(path("/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(info_body()))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/control"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/stream"))
        .respond_with(BurstyStream {
            opens: opens.clone(),
            packets_per_open: 1,
            samples_per_packet: 4,
        })
        .mount(&server)
        .await;

    let config = AaroniaConfig::from_http(&server.uri())
        .stream_format(StreamFormat::Json)
        .auto_reconnect(false)
        .read_timeout(Duration::from_secs(20));
    let mut source = AaroniaSource::new(config).await.expect("source");
    source.start_streaming().await.expect("start_streaming");

    // More samples than one connection can deliver: without reconnect
    // the reader task ends and the read reports a closed stream.
    let mut buffer = Vec::new();
    let result = source.read_samples(&mut buffer, 64).await;
    match result {
        Err(Error::Protocol(_)) => {}
        // A partial read is equally acceptable here — what must not
        // happen is a silent reconnect.
        Ok(_) => {}
        other => panic!("expected a closed-stream error or partial read, got {other:?}"),
    }
    assert_eq!(
        opens.load(Ordering::SeqCst),
        1,
        "auto_reconnect(false) must not reopen the stream"
    );
}
