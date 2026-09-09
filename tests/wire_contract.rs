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

/// The `#[repr(C)]` vendor mirror must keep its exact field list, in order.
///
/// This is the only guard for the quietest failure in the tree: renaming
/// *or reordering* a field of `AARTSAAPI_Packet` changes no size and no
/// alignment, so the crate compiles and every other test passes while the
/// struct stops matching `aaroniartsaapi.h`. Reordering is the worse of
/// the two — it silently reads the wrong bytes.
///
/// Note `step_frequency` is the vendor's name for the sample rate and
/// `span_frequency` is theirs for the span; neither may be "corrected" to
/// this crate's vocabulary.
#[test]
fn vendor_packet_struct_fields_are_intact() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/native_sdk.rs"),
    )
    .expect("src/native_sdk.rs");

    let at = src
        .find("pub struct AARTSAAPI_Packet {")
        .expect("AARTSAAPI_Packet must exist — it mirrors the vendor header");
    let open = src[at..].find('{').expect("struct body") + at;
    let close = src[open..].find('}').expect("struct end") + open;
    let body = &src[open + 1..close];

    let fields: Vec<String> = body
        .lines()
        .filter_map(|l| l.trim().strip_prefix("pub "))
        .filter_map(|r| r.split(':').next())
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect();

    let expected = [
        "cbsize",
        "stream_id",
        "flags",
        "start_time",
        "end_time",
        "start_frequency",
        "step_frequency",
        "span_frequency",
        "rbw_frequency",
        "num",
        "total",
        "size",
        "stride",
        "fp32",
        "interleave",
    ];

    assert_eq!(
        fields, expected,
        "AARTSAAPI_Packet no longer mirrors the vendor header field-for-field, in order. \
         A rename or reorder here breaks nothing the compiler can see."
    );
}

/// RTSA config-tree keys are typed by the device, not chosen by us.
///
/// Matched with their quotes so a mention in prose does not satisfy the
/// check — these must survive as string literals.
#[test]
fn rtsa_device_key_literals_are_intact() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/http_endpoints.rs"),
    )
    .expect("src/http_endpoints.rs");

    for key in ["centerfreq0", "reflevel0", "decimation0", "sclksource"] {
        let quoted = format!("\"{key}\"");
        assert!(
            src.contains(&quoted),
            "src/http_endpoints.rs no longer contains the string literal {quoted} — \
             renaming an RTSA config key silently stops addressing the device"
        );
    }
}
