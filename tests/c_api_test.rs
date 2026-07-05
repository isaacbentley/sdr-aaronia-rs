use sdr_aaronia_rs::c_api::{
    AaroniaFfiError, aaronia_endpoints_client_free, aaronia_endpoints_client_new,
    aaronia_get_error_message, aaronia_last_error, aaronia_source_build,
    aaronia_source_builder_center_frequency, aaronia_source_builder_free,
    aaronia_source_builder_http_source, aaronia_source_builder_new,
    aaronia_source_builder_reference_level, aaronia_source_builder_span_frequency,
    aaronia_string_free,
};
use std::ffi::{CStr, CString};

#[test]
fn test_c_api_builder_lifecycle() {
    unsafe {
        let builder = aaronia_source_builder_new();
        assert!(!builder.is_null());

        aaronia_source_builder_center_frequency(builder, 2.4e9);
        aaronia_source_builder_span_frequency(builder, 20e6);
        aaronia_source_builder_reference_level(builder, 0.0);

        let url = CString::new("http://example.com").unwrap();
        aaronia_source_builder_http_source(builder, url.as_ptr());

        // We could call build, but it will fail due to no mock server.
        // Let's just free the builder.
        aaronia_source_builder_free(builder);
    }
}

#[test]
fn test_c_api_build_failure_sets_error() {
    unsafe {
        let builder = aaronia_source_builder_new();

        let url = CString::new("http://invalid-url.local").unwrap();
        aaronia_source_builder_http_source(builder, url.as_ptr());

        let source = aaronia_source_build(builder);
        assert!(source.is_null()); // build fails

        let err_ptr = aaronia_last_error();
        assert!(!err_ptr.is_null());

        let err_msg = CStr::from_ptr(err_ptr).to_string_lossy();
        assert!(err_msg.contains("failed"));

        aaronia_string_free(err_ptr);
        aaronia_source_builder_free(builder);
    }
}

#[test]
fn test_c_api_endpoints_client_lifecycle() {
    unsafe {
        let url = CString::new("http://example.com").unwrap();
        let client = aaronia_endpoints_client_new(url.as_ptr());
        assert!(!client.is_null());

        aaronia_endpoints_client_free(client);
    }
}

#[test]
fn test_aaronia_get_error_message() {
    unsafe {
        let msg_ptr = aaronia_get_error_message(AaroniaFfiError::NullPointer);
        assert!(!msg_ptr.is_null());
        let msg = CStr::from_ptr(msg_ptr).to_string_lossy();
        assert_eq!(msg, "Null pointer provided");
        aaronia_string_free(msg_ptr);
    }
}
