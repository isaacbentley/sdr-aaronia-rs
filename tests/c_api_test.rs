use sdr_aaronia_rs::c_api::{
    FfiComplex, SpectranFfiError, spectran_endpoints_client_free, spectran_endpoints_client_new,
    spectran_get_error_message, spectran_last_error, spectran_sink_build,
    spectran_sink_builder_device_serial, spectran_sink_builder_free, spectran_sink_builder_new,
    spectran_sink_free, spectran_sink_get_master_stream_time_ns,
    spectran_sink_set_center_frequency_hz, spectran_sink_set_sample_rate_hz,
    spectran_sink_set_trans_gain_db, spectran_sink_write_samples, spectran_source_build,
    spectran_source_builder_center_frequency_hz, spectran_source_builder_free,
    spectran_source_builder_http_source, spectran_source_builder_listen_only,
    spectran_source_builder_native_options, spectran_source_builder_new,
    spectran_source_builder_read_timeout_us, spectran_source_builder_reference_level_dbm,
    spectran_source_builder_sample_rate_hz, spectran_source_read_samples,
    spectran_source_read_samples_dual, spectran_source_read_samples_dual_timeout,
    spectran_source_read_samples_timeout, spectran_source_rf_span_range,
    spectran_source_set_rf_span_hz, spectran_string_free,
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

#[test]
fn reads_and_writes_reject_unaligned_complex_buffers_and_allow_zero_sample_tx_sync() {
    let mut raw_bytes = [0u8; 32];
    let unaligned = unsafe { raw_bytes.as_mut_ptr().add(1) as *mut FfiComplex };
    unsafe {
        assert_eq!(
            spectran_source_read_samples(std::ptr::dangling_mut(), unaligned, 2),
            -1
        );
        assert_eq!(
            spectran_source_read_samples_timeout(std::ptr::dangling_mut(), unaligned, 2, u64::MAX),
            -1
        );

        let sink_builder = spectran_sink_builder_new();
        let serial = CString::new("V6-TX-0001").unwrap();
        spectran_sink_builder_device_serial(sink_builder, serial.as_ptr());
        let sink = spectran_sink_build(sink_builder);
        spectran_sink_builder_free(sink_builder);
        assert!(!sink.is_null());

        // Valid runtime setters succeed; NaN/non-positive values fail cleanly.
        assert!(matches!(
            spectran_sink_set_center_frequency_hz(sink, 2.45e9),
            SpectranFfiError::Success
        ));
        assert!(matches!(
            spectran_sink_set_center_frequency_hz(sink, f64::NAN),
            SpectranFfiError::InternalError
        ));
        assert!(matches!(
            spectran_sink_set_sample_rate_hz(sink, 10e6),
            SpectranFfiError::Success
        ));
        assert!(matches!(
            spectran_sink_set_trans_gain_db(sink, -15.0),
            SpectranFfiError::Success
        ));
        let mut master_ns: i64 = 0;
        assert!(!spectran_sink_get_master_stream_time_ns(
            sink,
            &mut master_ns
        ));

        // Non-zero write with NULL samples returns NullPointer.
        assert!(matches!(
            spectran_sink_write_samples(sink, 0, 0.0, 0.0, 0, std::ptr::null(), 4),
            SpectranFfiError::NullPointer
        ));
        // Non-zero write with unaligned samples is rejected before slice construction.
        assert!(matches!(
            spectran_sink_write_samples(sink, 0, 0.0, 0.0, 0, unaligned, 1),
            SpectranFfiError::InternalError
        ));
        // 0-sample sync packet with NULL samples passes pointer check (fails only because sink is uninitialized).
        assert!(matches!(
            spectran_sink_write_samples(sink, 0, 0.0, 0.0, 1, std::ptr::null(), 0),
            SpectranFfiError::InternalError
        ));

        spectran_sink_free(sink);
    }
}

#[test]
fn builder_extreme_timeout_listen_only_and_error_codes() {
    unsafe {
        let builder = spectran_source_builder_new();
        spectran_source_builder_read_timeout_us(builder, u64::MAX);
        spectran_source_builder_listen_only(builder, true);

        // Null pointers return NullPointer rather than InternalError.
        assert!(matches!(
            spectran_source_rf_span_range(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut()
            ),
            SpectranFfiError::NullPointer
        ));
        assert!(matches!(
            spectran_source_set_rf_span_hz(std::ptr::null_mut(), 20e6),
            SpectranFfiError::NullPointer
        ));
        assert!(matches!(
            spectran_source_builder_native_options(
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                0.0,
                std::ptr::null(),
                0,
            ),
            SpectranFfiError::NullPointer
        ));

        // Invalid UTF-8 returns InvalidString.
        let bad_utf8 = [0xffu8, 0x00u8];
        assert!(matches!(
            spectran_source_builder_native_options(
                builder,
                bad_utf8.as_ptr() as *const std::os::raw::c_char,
                std::ptr::null(),
                0.0,
                std::ptr::null(),
                0,
            ),
            SpectranFfiError::InvalidString
        ));

        spectran_source_builder_free(builder);
    }
}
