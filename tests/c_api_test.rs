use sdr_aaronia_rs::c_api::{
    SpectranFfiError, spectran_endpoints_client_free, spectran_endpoints_client_new,
    spectran_get_error_message, spectran_last_error, spectran_source_build,
    spectran_source_builder_center_frequency_hz, spectran_source_builder_free,
    spectran_source_builder_http_source, spectran_source_builder_new,
    spectran_source_builder_reference_level_dbm, spectran_source_builder_sample_rate_hz,
    spectran_string_free,
};
use std::ffi::{CStr, CString};

#[test]
fn test_c_api_builder_lifecycle() {
    unsafe {
        let builder = spectran_source_builder_new();
        assert!(!builder.is_null());

        spectran_source_builder_center_frequency_hz(builder, 2.4e9);
        spectran_source_builder_sample_rate_hz(builder, 20e6);
        spectran_source_builder_reference_level_dbm(builder, 0.0);

        let url = CString::new("http://example.com").unwrap();
        spectran_source_builder_http_source(builder, url.as_ptr());

        // We could call build, but it will fail due to no mock server.
        // Let's just free the builder.
        spectran_source_builder_free(builder);
    }
}

#[test]
fn test_c_api_build_failure_sets_error() {
    unsafe {
        let builder = spectran_source_builder_new();

        let url = CString::new("http://invalid-url.local").unwrap();
        spectran_source_builder_http_source(builder, url.as_ptr());

        let source = spectran_source_build(builder);
        assert!(source.is_null()); // build fails

        let err_ptr = spectran_last_error();
        assert!(!err_ptr.is_null());

        let err_msg = CStr::from_ptr(err_ptr).to_string_lossy();
        assert!(err_msg.contains("failed"));

        spectran_string_free(err_ptr);
        spectran_source_builder_free(builder);
    }
}

#[test]
fn test_c_api_endpoints_client_lifecycle() {
    unsafe {
        let url = CString::new("http://example.com").unwrap();
        let client = spectran_endpoints_client_new(url.as_ptr());
        assert!(!client.is_null());

        spectran_endpoints_client_free(client);
    }
}

#[test]
fn test_spectran_get_error_message() {
    unsafe {
        let msg_ptr = spectran_get_error_message(SpectranFfiError::NullPointer as i32);
        assert!(!msg_ptr.is_null());
        let msg = CStr::from_ptr(msg_ptr).to_string_lossy();
        assert_eq!(msg, "Null pointer provided");
        spectran_string_free(msg_ptr);
    }
}
