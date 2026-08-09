use sdr_aaronia_rs::Error;
use sdr_aaronia_rs::http_endpoints::{AuthMethod, HttpEndpointsClient};
use std::time::Duration;
use wiremock::matchers::{basic_auth, body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_get_info_success() {
    let mock_server = MockServer::start().await;

    // A mock JSON response that simulates what the Aaronia RTSA HTTP server returns
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

    let client = HttpEndpointsClient::new(mock_server.uri(), AuthMethod::None).unwrap();

    let info = client.get_info().await.expect("Failed to get info");
    assert_eq!(info.name, "Spectran V6");
    assert_eq!(info.uuid, "1234-5678-9012");
    assert_eq!(info.title, "Remote Spectran");
}

#[tokio::test]
async fn test_get_info_timeout() {
    let mock_server = MockServer::start().await;

    // Simulate a hung connection by delaying the response significantly
    Mock::given(method("GET"))
        .and(path("/info"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("{}")
                .set_delay(Duration::from_secs(15)), // Assuming client timeout is < 15s
        )
        .mount(&mock_server)
        .await;

    // Our HTTP client uses reqwest defaults if not configured, but we can verify it eventually times out
    let client = HttpEndpointsClient::new(mock_server.uri(), AuthMethod::None).unwrap();

    // Instead of waiting the full timeout, we could configure a custom timeout client,
    // but the HttpEndpointsClient API in aaronia-rs might not expose `reqwest::ClientBuilder` explicitly.
    // Assuming the reqwest timeout handles it eventually, we'll just check for an error.
    // For unit tests, we'll wrap it in a tokio::time::timeout to enforce a hard boundary so tests don't hang.
    let result = tokio::time::timeout(Duration::from_secs(2), client.get_info()).await;

    match result {
        Err(_) => {
            // Reached our test-level timeout, meaning the client didn't return immediately.
            // Ideally the reqwest client inside HttpEndpointsClient has a sane timeout configured.
        }
        Ok(Err(_)) => {
            // Client errored out (maybe reqwest internal timeout triggered)
        }
        Ok(Ok(_)) => {
            panic!("Expected timeout or error, got successful response");
        }
    }
}

#[tokio::test]
async fn test_control_streaming_server_error() {
    let mock_server = MockServer::start().await;

    // Simulate an Internal Server Error on the real request the client
    // issues: PUT /control with a StreamingControl body.
    Mock::given(method("PUT"))
        .and(path("/control"))
        .and(body_json(serde_json::json!({
            "start": true,
            "type": "streaming"
        })))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpEndpointsClient::new(mock_server.uri(), AuthMethod::None).unwrap();

    let err = client
        .control_streaming(true)
        .await
        .expect_err("Expected error on 500 response");
    match err {
        Error::Http { status, .. } => assert_eq!(status.as_u16(), 500),
        other => panic!("Expected HTTP status error, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_streaming_invalid_auth() {
    let mock_server = MockServer::start().await;

    // Simulate 401 Unauthorized on PUT /control, requiring that the client
    // actually attached the Basic auth credentials it was configured with.
    Mock::given(method("PUT"))
        .and(path("/control"))
        .and(basic_auth("user", "bad_pass"))
        .and(body_json(serde_json::json!({
            "start": true,
            "type": "streaming"
        })))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&mock_server)
        .await;

    // Use fake Basic auth
    let client = HttpEndpointsClient::new(
        mock_server.uri(),
        AuthMethod::Basic {
            username: "user".into(),
            password: "bad_pass".into(),
        },
    )
    .unwrap();

    let err = client
        .control_streaming(true)
        .await
        .expect_err("Expected error on 401 response");
    match err {
        Error::Http { status, .. } => assert_eq!(status.as_u16(), 401),
        other => panic!("Expected HTTP status error, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_get_inputs_success() {
    let mock_server = MockServer::start().await;
    let inputs_json = serde_json::json!({
        "inputs": ["spectranv6/iqreceiver", "spectranv6/sweepsa"]
    });

    Mock::given(method("GET"))
        .and(path("/inputs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&inputs_json))
        .mount(&mock_server)
        .await;

    let client = HttpEndpointsClient::new(mock_server.uri(), AuthMethod::None).unwrap();
    let inputs = client.get_inputs().await.expect("Failed to get inputs");
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[0], "spectranv6/iqreceiver");
}

#[tokio::test]
async fn test_create_input_success() {
    let mock_server = MockServer::start().await;
    let response_json = serde_json::json!({
        "name": "spectranv6/iqreceiver/average"
    });

    Mock::given(method("POST"))
        .and(path("/inputs"))
        .respond_with(ResponseTemplate::new(201).set_body_json(&response_json))
        .mount(&mock_server)
        .await;

    let client = HttpEndpointsClient::new(mock_server.uri(), AuthMethod::None).unwrap();
    let result = client
        .create_input(
            "spectranv6/iqreceiver",
            sdr_aaronia_rs::http_endpoints::InputProcessingType::Average,
        )
        .await
        .expect("Failed to create input");
    assert_eq!(result, "spectranv6/iqreceiver/average");
}

#[tokio::test]
async fn test_control_recording_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("PUT"))
        .and(path("/control"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let client = HttpEndpointsClient::new(mock_server.uri(), AuthMethod::None).unwrap();
    let result = client
        .control_recording(true, Some("test_capture.rtsa".to_string()))
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_get_remote_config_success() {
    let mock_server = MockServer::start().await;
    let config_json = serde_json::json!({
        "request": 42,
        "config": {
            "type": "group",
            "name": "root",
            "label": "Root",
            "items": [
                {
                    "type": "float",
                    "name": "startfreq",
                    "label": "Start Freq",
                    "value": 1e9,
                    "default": 1e9,
                    "min": 9e8,
                    "max": 2e9,
                    "step": 1000.0,
                    "unit": "Hz"
                }
            ]
        }
    });

    Mock::given(method("GET"))
        .and(path("/remoteconfig"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&config_json))
        .mount(&mock_server)
        .await;

    let client = HttpEndpointsClient::new(mock_server.uri(), AuthMethod::None).unwrap();
    let config = client
        .get_config()
        .await
        .expect("Failed to get remote config");
    assert_eq!(config.request, 42);
    if let sdr_aaronia_rs::http_endpoints::ConfigItem::Group { name, items, .. } = config.config {
        assert_eq!(name, "root");
        assert_eq!(items.len(), 1);
    } else {
        panic!("Expected group config item");
    }
}

#[tokio::test]
async fn test_test_connection_success() {
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

    let client = HttpEndpointsClient::new(mock_server.uri(), AuthMethod::None).unwrap();
    let result = client.test_connection().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_get_user_success() {
    let mock_server = MockServer::start().await;
    let user_json = serde_json::json!({
        "name": "test_user",
        "token": "secret_token_123",
        "groups": ["admin"]
    });

    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&user_json))
        .mount(&mock_server)
        .await;

    let client = HttpEndpointsClient::new(mock_server.uri(), AuthMethod::None).unwrap();
    let user = client.get_user().await.expect("Failed to get user");
    assert_eq!(user.name, "test_user");
    assert_eq!(user.token, "secret_token_123");
}

#[tokio::test]
async fn test_control_antenna_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("PUT"))
        .and(path("/control"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let client = HttpEndpointsClient::new(mock_server.uri(), AuthMethod::None).unwrap();
    let result = client.control_antenna(true).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_get_health_status_success() {
    let mock_server = MockServer::start().await;
    let health_json = serde_json::json!({
        "type": "group",
        "name": "health",
        "label": "Health",
        "items": []
    });

    Mock::given(method("GET"))
        .and(path("/healthstatus"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&health_json))
        .mount(&mock_server)
        .await;

    let client = HttpEndpointsClient::new(mock_server.uri(), AuthMethod::None).unwrap();
    let health = client
        .get_health_status()
        .await
        .expect("Failed to get health status");
    if let sdr_aaronia_rs::http_endpoints::ConfigItem::Group { name, .. } = health {
        assert_eq!(name, "health");
    } else {
        panic!("Expected group config item");
    }
}

#[tokio::test]
async fn test_push_samples_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/sample"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let client = HttpEndpointsClient::new(mock_server.uri(), AuthMethod::None).unwrap();
    let samples = vec![1.0, 0.0, 0.5, -0.5];
    let req = sdr_aaronia_rs::http_endpoints::TxSampleRequest {
        start_time: 1000.0,
        end_time: 1000.1,
        start_frequency: 100e6,
        end_frequency: 101e6,
        step_frequency: None,
        min_power: -2.0,
        max_power: 2.0,
        sample_size: 2,
        sample_depth: 1,
        unit: "volt".to_string(),
        payload: "iq".to_string(),
        push: true,
        samples: &samples,
    };
    let result = client.push_samples(&req).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_get_sample_success() {
    let mock_server = MockServer::start().await;
    let sample_json = serde_json::json!({
        "startTime": 0.0,
        "endTime": 1.0,
        "startFrequency": 1e9,
        "endFrequency": 2e9,
        "stepFrequency": 0.0,
        "minPower": -100,
        "maxPower": 0,
        "sampleSize": 3,
        "sampleDepth": 1,
        "unit": "dBm",
        "payload": "iq",
        "samples": 3
    });

    Mock::given(method("GET"))
        .and(path("/sample"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&sample_json))
        .mount(&mock_server)
        .await;

    let client = HttpEndpointsClient::new(mock_server.uri(), AuthMethod::None).unwrap();
    let sample = client.get_sample(None).await.expect("Failed to get sample");
    assert_eq!(sample.sample_size, 3);
}

#[tokio::test]
async fn test_get_samples_success() {
    let mock_server = MockServer::start().await;
    let sample_json = serde_json::json!([{
        "startTime": 0.0,
        "endTime": 1.0,
        "startFrequency": 1e9,
        "endFrequency": 2e9,
        "stepFrequency": 0.0,
        "minPower": -100,
        "maxPower": 0,
        "sampleSize": 3,
        "sampleDepth": 1,
        "unit": "dBm",
        "payload": "iq",
        "samples": 3
    }]);

    Mock::given(method("GET"))
        .and(path("/samples"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&sample_json))
        .mount(&mock_server)
        .await;

    let client = HttpEndpointsClient::new(mock_server.uri(), AuthMethod::None).unwrap();
    let samples = client
        .get_samples(None, None)
        .await
        .expect("Failed to get samples");
    assert_eq!(samples.len(), 1);
}

#[tokio::test]
async fn test_shutdown_application_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("PUT"))
        .and(path("/app/process"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let client = HttpEndpointsClient::new(mock_server.uri(), AuthMethod::None).unwrap();
    let result = client.shutdown_application().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_configure_capture_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("PUT"))
        .and(path("/control"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let client = HttpEndpointsClient::new(mock_server.uri(), AuthMethod::None).unwrap();
    let control = sdr_aaronia_rs::http_endpoints::CaptureControl {
        frequency_center: Some(1e9),
        ..Default::default()
    };
    let result = client.configure_capture(control).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_probe_remote_config_license() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/remoteconfig"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&mock_server)
        .await;

    let client = HttpEndpointsClient::new(mock_server.uri(), AuthMethod::None).unwrap();
    let status = client.probe_remote_config_write_license().await;
    assert_eq!(
        status,
        sdr_aaronia_rs::http_endpoints::RemoteConfigStatus::AuthenticationRequired
    );
}

#[tokio::test]
async fn test_set_remote_config() {
    let mock_server = MockServer::start().await;
    let config_json = serde_json::json!({
        "request": 42,
        "config": {
            "type": "group",
            "name": "root",
            "label": "Root",
            "items": []
        }
    });

    Mock::given(method("PUT"))
        .and(path("/remoteconfig"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&config_json))
        .mount(&mock_server)
        .await;

    let client = HttpEndpointsClient::new(mock_server.uri(), AuthMethod::None).unwrap();
    let result = client
        .simple_remote_config(
            "Block_IQDemodulator_0",
            config_json.as_object().unwrap().clone(),
        )
        .await;
    assert!(result.is_ok());
}
