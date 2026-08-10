use futuresdr::blocks::VectorSource;
use futuresdr::macros::connect;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Runtime;
use num_complex::Complex32;
use sdr_aaronia_rs::http_endpoints::AuthMethod;
use sdr_aaronia_rs::http_sink::HttpSinkBuilder;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Hard deadline for a flowgraph run. These graphs process a handful of
/// samples and normally finish in milliseconds; the guard exists because
/// an intermittent Windows-only hang in this suite once wedged a CI
/// runner for ~2 hours (run 31352924497) with no output past "running 3
/// tests". A hang should be a fast, attributable failure, not a stuck
/// runner.
const FLOWGRAPH_DEADLINE: Duration = Duration::from_secs(60);

/// Run `fg` to completion under [`FLOWGRAPH_DEADLINE`].
///
/// The callers use a multi-thread tokio runtime so this timer (and the
/// wiremock server the sink talks to) keeps running even if polling the
/// FutureSDR future ever blocks its worker thread — the suspected shape
/// of the CI hang, which a timeout on a single-threaded runtime could
/// not interrupt.
async fn run_flowgraph_with_deadline(fg: Flowgraph) {
    tokio::time::timeout(FLOWGRAPH_DEADLINE, Runtime::new().run_async(fg))
        .await
        .expect("flowgraph did not terminate within the hang-guard deadline")
        .expect("Flowgraph execution failed");
}

#[tokio::test]
async fn test_http_sink_builder() {
    let builder = HttpSinkBuilder::new("http://example.com:8080")
        .frequency(2.4e9)
        .sample_rate(20e6)
        .buffer_size(1024)
        .streaming_delay(0.5)
        .auth(AuthMethod::None);

    let sink = builder.build().expect("Failed to build HttpSink");
    assert_eq!(sink.dropped_samples(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_http_sink_builder_and_flowgraph() -> anyhow::Result<()> {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/sample"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    // Buffer size 5, pushing 10 samples -> 2 requests
    let sink = HttpSinkBuilder::new(&mock_server.uri())
        .buffer_size(5)
        .build()
        .unwrap();

    let samples: Vec<Complex32> = (0..10)
        .map(|i| Complex32::new(i as f32, -i as f32))
        .collect();
    let src = VectorSource::<Complex32>::new(samples);

    let mut fg = Flowgraph::new();
    connect!(fg, src > sink);

    run_flowgraph_with_deadline(fg).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_http_sink_work_server_error() -> anyhow::Result<()> {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/sample"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    let sink = HttpSinkBuilder::new(&mock_server.uri())
        .buffer_size(5)
        .build()
        .unwrap();

    let samples: Vec<Complex32> = (0..5)
        .map(|i| Complex32::new(i as f32, -i as f32))
        .collect();
    let src = VectorSource::<Complex32>::new(samples);

    let mut fg = Flowgraph::new();
    connect!(fg, src > sink);

    run_flowgraph_with_deadline(fg).await;
    Ok(())
}
