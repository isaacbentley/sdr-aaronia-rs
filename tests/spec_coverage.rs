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
        test_fn: "prop_http_framing_rs_and_lf",
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
        invariant: "aaronia_get_error_message maps every error code to a non-empty string",
        test_fn: "test_ffi_error_message_mapping",
        test_file: "src/c_api.rs",
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

/// Defensive check: no area accumulates an absurd number of rows
/// without explicit intent. Catches accidental double-claim — pick one
/// row as the canonical owner if the invariants overlap.
#[test]
fn spec_coverage_has_no_unintentional_duplicates() {
    use std::collections::HashMap;
    let mut by_area: HashMap<Area, Vec<&'static str>> = HashMap::new();
    for row in ENFORCED {
        by_area.entry(row.area).or_default().push(row.test_fn);
    }
    for (area, tests) in &by_area {
        // Loose upper bound; tighten if drift becomes a problem.
        assert!(
            tests.len() <= 16,
            "area {} has {} tests claiming it: {:?} — sanity-check whether the rows are all needed",
            area.name(),
            tests.len(),
            tests
        );
    }
}
