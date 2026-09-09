//! The wire contract: JSON keys that leave this process, and the vendor
//! struct fields that mirror `aaroniartsaapi.h`.
//!
//! These are written against the names as they stand *before* the 0.10.0
//! rename, so that a failure here later is unambiguously the rename's
//! fault rather than a pre-existing bug.
//!
//! Why keys and not just values: `#[serde(rename_all = "camelCase")]`
//! derives every JSON key from the Rust field name, so renaming a field
//! silently moves the wire. On an `Option` field the failure is quieter
//! than a parse error — an unrecognised key simply deserialises to `None`,
//! and a sample rate becomes "absent" instead of wrong.

use sdr_aaronia_rs::http_endpoints::{CaptureControl, ControlType, TxSampleRequest};
use sdr_aaronia_rs::http_streaming::PacketMetadata;
use std::collections::BTreeSet;

/// The set of top-level JSON keys a value serialises to.
fn keys_of<T: serde::Serialize>(value: &T) -> BTreeSet<String> {
    let json = serde_json::to_value(value).expect("value must serialise");
    json.as_object()
        .expect("expected a JSON object")
        .keys()
        .cloned()
        .collect()
}

/// `/control` capture payload. RTSA-Suite reads these exact keys; the
/// device answers 200 to a payload it does not understand, so a wrong key
/// is silent.
#[test]
fn capture_control_wire_keys_are_pinned() {
    let cmd = CaptureControl {
        frequency_center: Some(1920e6),
        frequency_span: Some(200e6),
        frequency_start: Some(1820e6),
        frequency_end: Some(2020e6),
        frequency_bins: Some(448),
        reference_level: Some(-20.0),
        control_type: ControlType::Capture,
    };

    let expected: BTreeSet<String> = [
        "frequencyCenter",
        "frequencySpan",
        "frequencyStart",
        "frequencyEnd",
        "frequencyBins",
        "referenceLevel",
        "type",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    assert_eq!(
        keys_of(&cmd),
        expected,
        "CaptureControl's JSON keys are the /control wire contract"
    );
}

/// `/sample` TX push payload.
#[test]
fn tx_sample_request_wire_keys_are_pinned() {
    let samples = [0.0f32, 1.0, 0.0, -1.0];
    let req = TxSampleRequest {
        start_time: 1.0,
        end_time: 2.0,
        start_frequency: 2.4e9,
        end_frequency: 2.5e9,
        step_frequency: Some(1e6),
        min_power: -120.0,
        max_power: 10.0,
        sample_size: 2,
        sample_depth: 1,
        unit: "volt".to_string(),
        payload: "iq".to_string(),
        push: true,
        samples: &samples,
    };

    let expected: BTreeSet<String> = [
        "startTime",
        "endTime",
        "startFrequency",
        "endFrequency",
        "stepFrequency",
        "minPower",
        "maxPower",
        "sampleSize",
        "sampleDepth",
        "unit",
        "payload",
        "push",
        "samples",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    assert_eq!(
        keys_of(&req),
        expected,
        "TxSampleRequest's JSON keys are the /sample wire contract"
    );
}

/// The `/stream` packet header, verbatim from `docs/HTTPSPEC.md`.
///
/// The `Option` fields are the point: rename one and it deserialises to
/// `None` rather than failing, so a stream would silently lose its sample
/// rate or scale factor.
#[test]
fn packet_metadata_parses_the_documented_header_including_optionals() {
    let header = serde_json::json!({
        "startTime": 1501163970.1396854,
        "endTime": 1501163970.140799,
        "startTimeDay": 17372,
        "endTimeDay": 17372,
        "startFrequency": 2400000000.0,
        "endFrequency": 2500000000.0,
        "sampleFrequency": 100000000.0,
        "samples": 1024,
        "unit": "dbm",
        "payload": "iq",
        "minPower": -120,
        "maxPower": 10,
        "sampleSize": 2,
        "sampleDepth": 1,
        "scale": 16384
    });

    let meta: PacketMetadata =
        serde_json::from_value(header).expect("the documented header must parse");

    assert_eq!(meta.start_frequency, 2.4e9);
    assert_eq!(meta.end_frequency, 2.5e9);
    assert_eq!(meta.sample_size, 2);
    assert_eq!(meta.samples, 1024);

    // Every optional the header carries must arrive as `Some`. A renamed
    // field would land here as `None` and nowhere else.
    assert_eq!(
        meta.sample_frequency,
        Some(1e8),
        "sampleFrequency must not silently become None"
    );
    assert_eq!(meta.start_time_day, Some(17372.0), "startTimeDay");
    assert_eq!(meta.end_time_day, Some(17372.0), "endTimeDay");
    assert_eq!(meta.sample_depth, Some(1), "sampleDepth");
    assert_eq!(meta.scale, Some(16384.0), "scale");
}

/// Literals that must survive any rename, checked in the source itself.
///
/// This is the only guard that catches a search-and-replace reaching the
/// `#[repr(C)]` vendor mirror: renaming a field there changes no layout,
/// so the crate still compiles and every other test still passes while the
/// correspondence to `aaroniartsaapi.h` is quietly destroyed.
#[test]
fn vendor_and_device_key_literals_are_intact() {
    let root = env!("CARGO_MANIFEST_DIR");
    let cases: &[(&str, &[&str])] = &[
        (
            "src/native_sdk.rs",
            // AARTSAAPI_Packet mirrors the vendor header field-for-field.
            // `step_frequency` is the vendor's name for the sample rate.
            &["span_frequency", "step_frequency", "start_frequency"],
        ),
        (
            "src/http_endpoints.rs",
            // RTSA config-tree keys, typed by the device, not by us.
            &["centerfreq0", "reflevel0", "decimation0", "sclksource"],
        ),
    ];

    for (rel, literals) in cases {
        let path = std::path::Path::new(root).join(rel);
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        for lit in *literals {
            assert!(
                src.contains(lit),
                "{rel} no longer contains `{lit}` — if a rename moved it, the vendor \
                 struct or an RTSA wire key has been broken silently"
            );
        }
    }
}
