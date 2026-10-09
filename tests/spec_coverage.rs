//! Invariant-coverage inventory.
//!
//! Single index of which testable invariants are enforced by the test
//! suite, and which test function enforces each one. Adding a new
//! invariant-bound test? Add a row below.
//!
//! Run `cargo test --test spec_coverage -- --nocapture` for a coverage
//! summary. Provides a grep-and-eyeball way to answer "are we still
//! enforcing the invariants we claim to enforce?".

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Area {
    FileFormat,
    HttpProtocol,
    Sdk,
}

impl Area {
    fn name(self) -> &'static str {
        match self {
            Area::FileFormat => "File format",
            Area::HttpProtocol => "HTTP protocol",
            Area::Sdk => "SDK",
        }
    }
}

/// One row per documented invariant that the test suite pins.
///
/// Format: `(area, invariant summary, enforcing test fn name)`. The
/// `test_fn` field is the *name* — not a function pointer — so this
/// file compiles even if a test is gated behind a feature flag.
struct InvariantRow {
    area: Area,
    invariant: &'static str,
    test_fn: &'static str,
    test_file: &'static str,
}

const ENFORCED: &[InvariantRow] = &[
    InvariantRow {
        area: Area::Sdk,
        invariant: "explicit sweep reference levels are finite and bounded before any write",
        test_fn: "sweep_reference_level_is_checked_before_any_write",
        test_file: "src/native_sdk_lifecycle_tests.rs",
    },
    InvariantRow {
        area: Area::Sdk,
        invariant: "missing explicit sweep reference metadata fails before geometry writes",
        test_fn: "missing_explicit_sweep_reference_metadata_fails_before_writes",
        test_file: "src/native_sdk_lifecycle_tests.rs",
    },
    InvariantRow {
        area: Area::Sdk,
        invariant: "Teardown attempts stop, disconnect, close; errors are not swallowed",
        test_fn: "stop_attempts_every_cleanup_stage_and_returns_first_failure",
        test_file: "src/native_sdk_lifecycle_tests.rs",
    },
    InvariantRow {
        area: Area::Sdk,
        invariant: "Failed startup releases partial device ownership",
        test_fn: "failed_start_releases_partial_connection",
        test_file: "src/native_sdk_lifecycle_tests.rs",
    },
    InvariantRow {
        area: Area::Sdk,
        invariant: "An opened device is closed even if streaming never starts",
        test_fn: "drop_closes_an_open_device_that_never_started",
        test_file: "src/native_sdk_lifecycle_tests.rs",
    },
    InvariantRow {
        area: Area::Sdk,
        invariant: "Process-wide SDK shutdown waits for the last initialized client",
        test_fn: "sdk_shutdown_waits_for_the_last_initialized_client",
        test_file: "src/native_sdk_lifecycle_tests.rs",
    },
    InvariantRow {
        area: Area::Sdk,
        invariant: "Disabled SDK options preserve their original enum indices",
        test_fn: "disabled_sdk_options_keep_their_indices",
        test_file: "src/capabilities.rs",
    },
    InvariantRow {
        area: Area::Sdk,
        invariant: "Undocumented payload modes cannot be interpreted as IQ",
        test_fn: "unknown_modes_are_not_reinterpreted_as_iq_or_spectra",
        test_file: "src/native_sdk.rs",
    },
    InvariantRow {
        area: Area::Sdk,
        invariant: "IQ sample clock and valid RF width are independent; centre is start + span / 2",
        test_fn: "observed_usb_rate_does_not_replace_rf_width_or_shift_the_center",
        test_file: "src/stream_geometry.rs",
    },
    InvariantRow {
        area: Area::Sdk,
        invariant: "RF geometry supports distinct model widths without a universal 44 MHz ratio",
        test_fn: "geometry_is_not_limited_to_one_spectran_model",
        test_file: "src/stream_geometry.rs",
    },
    InvariantRow {
        area: Area::Sdk,
        invariant: "Malformed RF metadata remains unknown",
        test_fn: "invalid_geometry_remains_unknown",
        test_file: "src/stream_geometry.rs",
    },
    InvariantRow {
        area: Area::Sdk,
        invariant: "ECO IQ span is validated against device-reported bounds",
        test_fn: "eco_bounds_come_from_the_device",
        test_file: "src/stream_geometry.rs",
    },
    InvariantRow {
        area: Area::FileFormat,
        invariant: "DSFH::mCreationTime stored in microseconds is normalised to seconds",
        test_fn: "prop_creation_time_normalization",
        test_file: "tests/properties.rs",
    },
    InvariantRow {
        area: Area::FileFormat,
        invariant: "compression_factor == 0 indicates uncompressed payload",
        test_fn: "prop_decompress_factor_zero_returns_error",
        test_file: "tests/properties.rs",
    },
    InvariantRow {
        area: Area::HttpProtocol,
        invariant: "framing accepts both 0x1E (RS) and 0x0A (LF) separators",
        test_fn: "prop_http_framing_separators",
        test_file: "tests/properties.rs",
    },
    InvariantRow {
        area: Area::HttpProtocol,
        invariant: "framing consumes exactly one separator byte (no run-skip)",
        test_fn: "regression_int16_sample_starts_with_rs_byte",
        test_file: "tests/properties.rs",
    },
    InvariantRow {
        area: Area::HttpProtocol,
        invariant: "framing accepts an int16 sample whose first byte is 0x0A",
        test_fn: "regression_int16_sample_starts_with_lf_byte",
        test_file: "tests/properties.rs",
    },
    InvariantRow {
        area: Area::HttpProtocol,
        invariant: "int16 samples decode as f32 = scale * raw_i16",
        test_fn: "prop_int16_scale_roundtrip",
        test_file: "tests/properties.rs",
    },
    InvariantRow {
        area: Area::Sdk,
        invariant: "IQ-Mode constraint: spanfreq * 1.5 <= receiverclock",
        test_fn: "prop_validate_iq_mode_boundary",
        test_file: "tests/properties.rs",
    },
    InvariantRow {
        area: Area::HttpProtocol,
        invariant: "float16 IQ samples decode to expected Complex32 within precision",
        test_fn: "test_stream_parser_f16_format",
        test_file: "tests/integration_test.rs",
    },
    InvariantRow {
        area: Area::HttpProtocol,
        invariant: "partial JSON header buffers and waits rather than erroring",
        test_fn: "test_stream_parser_invalid_format",
        test_file: "tests/integration_test.rs",
    },
    InvariantRow {
        area: Area::HttpProtocol,
        invariant: "PacketMetadata.samples deserializes from a numeric count (binary streams)",
        test_fn: "samples_field_deserializes_from_count",
        test_file: "src/http_streaming.rs",
    },
    InvariantRow {
        area: Area::HttpProtocol,
        invariant: "PacketMetadata.samples deserializes from a JSON array (JSON streams)",
        test_fn: "samples_field_deserializes_from_array",
        test_file: "src/http_streaming.rs",
    },
    InvariantRow {
        area: Area::HttpProtocol,
        invariant: "JSON IQ stream decodes to exact Complex32 values",
        test_fn: "test_stream_parser_json_format",
        test_file: "tests/integration_test.rs",
    },
    InvariantRow {
        area: Area::HttpProtocol,
        invariant: "wiremock: /info returns parsed ServerInfo on 200",
        test_fn: "test_get_info_success",
        test_file: "tests/http_mock_test.rs",
    },
    InvariantRow {
        area: Area::HttpProtocol,
        invariant: "wiremock: slow server triggers client timeout cleanly",
        test_fn: "test_get_info_timeout",
        test_file: "tests/http_mock_test.rs",
    },
    InvariantRow {
        area: Area::HttpProtocol,
        invariant: "wiremock: 5xx mid-request surfaces as Err",
        test_fn: "test_control_streaming_server_error",
        test_file: "tests/http_mock_test.rs",
    },
    InvariantRow {
        area: Area::HttpProtocol,
        invariant: "wiremock: 401 with bad Basic auth surfaces as Err",
        test_fn: "test_streaming_invalid_auth",
        test_file: "tests/http_mock_test.rs",
    },
    InvariantRow {
        area: Area::FileFormat,
        invariant: "RtsaSource::open rejects garbage bytes without panicking",
        test_fn: "test_rtsa_invalid_signature",
        test_file: "tests/rtsa_negative_test.rs",
    },
    InvariantRow {
        area: Area::FileFormat,
        invariant: "RtsaSource::open rejects truncated DSFH header without panicking",
        test_fn: "test_rtsa_truncated_dsfh_header",
        test_file: "tests/rtsa_negative_test.rs",
    },
    InvariantRow {
        area: Area::FileFormat,
        invariant: "RtsaSource::open handles unknown chunk types gracefully",
        test_fn: "test_rtsa_invalid_chunk_type",
        test_file: "tests/rtsa_negative_test.rs",
    },
    InvariantRow {
        area: Area::FileFormat,
        invariant: "decompress round-trips a hand-crafted Rice bitstream to exact coefficients",
        test_fn: "test_decompression_exact_oracle",
        test_file: "src/decompression.rs",
    },
    InvariantRow {
        area: Area::Sdk,
        invariant: "C FFI builder lifecycle is null-safe and double-free safe",
        test_fn: "test_ffi_builder_lifecycle",
        test_file: "src/c_api.rs",
    },
    InvariantRow {
        area: Area::Sdk,
        invariant: "C FFI endpoints accept null pointers without UB",
        test_fn: "test_ffi_null_pointer_handling",
        test_file: "src/c_api.rs",
    },
    InvariantRow {
        area: Area::Sdk,
        invariant: "spectran_get_error_message maps every error code to a non-empty string",
        test_fn: "test_ffi_error_message_mapping",
        test_file: "src/c_api.rs",
    },
    InvariantRow {
        area: Area::FileFormat,
        invariant: "STRT has 4 bytes of alignment padding before the 8-byte-aligned mEndTime",
        test_fn: "test_strt_chunk_parsing",
        test_file: "src/file_source.rs",
    },
    InvariantRow {
        area: Area::FileFormat,
        invariant: "STRT tail offsets are size-versioned; absent offsets default to 0",
        test_fn: "test_strt_chunk_parsing_88_byte_no_metadata_offset",
        test_file: "src/file_source.rs",
    },
    InvariantRow {
        area: Area::FileFormat,
        invariant: "STRM uses the standard layout at every chunk size, including 40 bytes",
        test_fn: "test_strm_chunk_40_byte_standard_format",
        test_file: "src/file_source.rs",
    },
    InvariantRow {
        area: Area::FileFormat,
        invariant: "DSST numbering matches the official enum (S16=2, U32=3, U32N=9)",
        test_fn: "test_dsp_stream_sample_type_conversion",
        test_file: "src/file_source.rs",
    },
    InvariantRow {
        area: Area::FileFormat,
        invariant: "out-of-range SAMP enum values degrade to Unknown instead of failing the open",
        test_fn: "test_invalid_chunk_handling",
        test_file: "src/file_source.rs",
    },
    InvariantRow {
        area: Area::Sdk,
        invariant: "RxChannel maps to the exact device/receiverchannel config strings",
        test_fn: "test_rx_channel_config_strings",
        test_file: "src/utils.rs",
    },
    InvariantRow {
        area: Area::Sdk,
        invariant: "dual-channel packets demux [I1 Q1 I2 Q2] per stride into two aligned streams",
        test_fn: "test_deinterleave_dual_iq_padded_stride",
        test_file: "src/utils.rs",
    },
    InvariantRow {
        area: Area::Sdk,
        invariant: "sub-dual stride is rejected with a receiverchannel hint, not misread",
        test_fn: "test_deinterleave_dual_iq_rejects_narrow_stride",
        test_file: "src/utils.rs",
    },
];

/// Print a per-area coverage summary. Intentionally has no assertions
/// other than "the table is non-empty"; the value of this test is the
/// printed report that ships with every CI log.
#[test]
fn spec_coverage_summary() {
    use std::collections::BTreeMap;

    assert!(
        !ENFORCED.is_empty(),
        "ENFORCED is empty — at least one invariant must be claimed"
    );

    let mut by_area: BTreeMap<&'static str, Vec<&InvariantRow>> = BTreeMap::new();
    for row in ENFORCED {
        by_area.entry(row.area.name()).or_default().push(row);
    }

    println!();
    println!("=== Aaronia-rs invariant coverage ===");
    println!("Total invariants enforced: {}", ENFORCED.len());
    for (area, rows) in &by_area {
        println!("  {} ({} invariants):", area, rows.len());
        for row in rows {
            println!(
                "    {} ({} :: {})",
                row.invariant, row.test_file, row.test_fn
            );
        }
    }
    println!("============================================");
}

/// Each invariant and enforcing test has one canonical inventory row. Check
/// actual duplicate claims rather than imposing a count ceiling on an area.
#[test]
fn spec_coverage_has_no_unintentional_duplicates() {
    use std::collections::HashSet;
    let mut invariants = HashSet::new();
    let mut tests = HashSet::new();
    for row in ENFORCED {
        assert!(
            invariants.insert((row.area, row.invariant)),
            "duplicate invariant: {}",
            row.invariant
        );
        assert!(
            tests.insert((row.test_file, row.test_fn)),
            "duplicate enforcing test: {}",
            row.test_fn
        );
    }
}
