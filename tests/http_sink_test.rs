use futuresdr::blocks::VectorSource;
use futuresdr::macros::connect;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Runtime;
use num_complex::Complex32;
use sdr_aaronia_rs::http_endpoints::AuthMethod;
use sdr_aaronia_rs::http_sink::HttpSinkBuilder;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

#[tokio::test]
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

    Runtime::new()
        .run_async(fg)
        .await
        .expect("Flowgraph execution failed");
    Ok(())
}

#[tokio::test]
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

    Runtime::new()
        .run_async(fg)
        .await
        .expect("Flowgraph execution failed");
    Ok(())
}
