use sdr_aaronia_rs::c_api::{
    FfiComplex, SpectranFfiError, spectran_endpoints_client_free, spectran_endpoints_client_new,
    spectran_get_error_message, spectran_last_error, spectran_source_build,
    spectran_source_builder_center_frequency_hz, spectran_source_builder_free,
    spectran_source_builder_http_source, spectran_source_builder_new,
    spectran_source_builder_reference_level_dbm, spectran_source_builder_sample_rate_hz,
    spectran_source_read_samples_dual, spectran_source_read_samples_dual_timeout,
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

/// The dual reads must reject a missing second buffer rather than
/// writing through it. `readStream` hands `buffs[1]` straight down when
/// an application sets up two channels, and a caller that asked for two
/// channels but supplied one buffer is exactly the mistake worth
/// catching at the boundary — not one page fault later.
#[test]
fn dual_reads_reject_a_null_buffer() {
    let mut buf: [FfiComplex; 4] = std::array::from_fn(|_| FfiComplex { re: 0.0, im: 0.0 });
    let p = buf.as_mut_ptr();
    unsafe {
        // The source pointer is dangling on purpose. A *null* one would
        // short-circuit the guard before the buffers are ever examined,
        // so the buffer checks would go untested; a non-null one reaches
        // them. Nothing dereferences it, because every case below has at
        // least one null buffer — and if a future edit moves the
        // dereference ahead of the guard, this test crashes rather than
        // passing quietly, which is the regression worth catching.
        for (rx1, rx2) in [
            (std::ptr::null_mut(), p),
            (p, std::ptr::null_mut()),
            (std::ptr::null_mut(), std::ptr::null_mut()),
        ] {
            assert_eq!(
                spectran_source_read_samples_dual(std::ptr::dangling_mut(), rx1, rx2, 4),
                -1
            );
            assert_eq!(
                spectran_source_read_samples_dual_timeout(std::ptr::dangling_mut(), rx1, rx2, 4, 0),
                -1
            );
        }
    }
}
