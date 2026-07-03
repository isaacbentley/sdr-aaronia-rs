//! Property-based tests pinning the documented invariants of the
//! parser, decompressor, and validator surfaces.
//!
//! Each `prop_*` test maps to one invariant and to the corresponding
//! bug from the prior code review (A1–A7). The cases are deliberately
//! narrow — one invariant per property — so a regression pinpoints the
//! failure to a single line rather than a broad "something broke"
//! failure.
//!
//! Run with `cargo test --test properties`. Set `PROPTEST_CASES=4096`
//! to widen the search space for nightly / release verification.

use bytes::Bytes;
use proptest::prelude::*;
use sdr_aaronia_rs::decompression::Decompressor;
use sdr_aaronia_rs::file_source::rtsa_epoch_seconds;
use sdr_aaronia_rs::http_streaming::{StreamFormat, StreamParser};
use sdr_aaronia_rs::utils::validate_iq_mode;

/// Build a synthetic HTTP stream packet with the canonical metadata header,
/// record-separator framing (`0x1E`), and the caller-supplied binary
/// payload. Mirrors `wrap_mock_packet` in `tests/integration_test.rs`
/// but lives here so this file is self-contained.
fn wrap_packet(payload: &[u8], payload_type: &str, samples: u64) -> Bytes {
    wrap_packet_sep(payload, payload_type, samples, &[0x1E])
}

/// As `wrap_packet`, but lets the caller pick the separator byte(s).
/// Live SpectranV6 hardware frames packets as `{json}\n\x1e<binary>`
/// (LF **and** RS — two bytes); a lone RS or lone LF is also accepted
/// for spec-conservative peers. Used by `prop_http_framing_separators`.
fn wrap_packet_sep(payload: &[u8], payload_type: &str, samples: u64, sep: &[u8]) -> Bytes {
    let json_meta = format!(
        r#"{{"startTime":0.0,"endTime":1.0,"startFrequency":0.0,"endFrequency":1.0,"samples":{},"unit":"volt","payload":"{}","minPower":0,"maxPower":1,"sampleSize":1}}"#,
        samples, payload_type
    );
    let mut data = json_meta.into_bytes();
    data.extend_from_slice(sep);
    data.extend_from_slice(payload);
    Bytes::from(data)
}

// -------------------------------------------------------------------------
// Pins A1: int16 sample scale factor must be applied.
//
// The `scale` (per-packet field or `?scale=N` query) is the server-side
// **encode multiplier** — `int16 = round(value * scale)` — so decoding
// divides: `f32 = raw_i16 / scale`. Verified on live hardware: a dBm
// spectra stream with `scale: 100` carries raw values like `-11378`
// = −113.78 dBm. This property feeds random `(scale, raw_re, raw_im)`
// triples through `StreamParser` and asserts the decoded `Complex32`
// matches `raw / scale` to within float-precision tolerance.
// -------------------------------------------------------------------------
proptest! {
    #[test]
    fn prop_int16_scale_roundtrip(
        // Realistic encode multipliers (live devices use e.g. 100, 16384).
        scale in 1.0_f64..1.0e6_f64,
        raw_re in i16::MIN..=i16::MAX,
        raw_im in i16::MIN..=i16::MAX,
    ) {
        let mut parser = StreamParser::new(StreamFormat::Int16, Some(scale)).unwrap();

        let mut binary = Vec::with_capacity(4);
        binary.extend_from_slice(&raw_re.to_le_bytes());
        binary.extend_from_slice(&raw_im.to_le_bytes());

        let packet = wrap_packet(&binary, "iq", 1);
        let result = parser.process_data(&packet).expect("parse should succeed");

        prop_assert_eq!(result.len(), 1);
        prop_assert_eq!(result[0].samples.len(), 1);

        let expected_re = (raw_re as f64 / scale) as f32;
        let expected_im = (raw_im as f64 / scale) as f32;

        // Relative tolerance with absolute floor — i16 / f32 has ~7
        // decimal digits of precision; small expected values dominated
        // by additive noise need the floor.
        let tol_re = 1.0e-9 + 1.0e-5 * expected_re.abs();
        let tol_im = 1.0e-9 + 1.0e-5 * expected_im.abs();
        prop_assert!(
            (result[0].samples[0].re - expected_re).abs() < tol_re,
            "re: got {} expected {} (tol {})",
            result[0].samples[0].re, expected_re, tol_re
        );
        prop_assert!(
            (result[0].samples[0].im - expected_im).abs() < tol_im,
            "im: got {} expected {} (tol {})",
            result[0].samples[0].im, expected_im, tol_im
        );
    }
}

// -------------------------------------------------------------------------
// Pins A4: compression_factor == 0 means uncompressed.
//
// `compression_factor == 0` indicates an uncompressed payload — there
// is no Rice/wavelet codestream to decode. The current
// `Decompressor::decompress` contract is "caller guards; passing 0 here
// is a precondition violation and returns an error". This property
// confirms the rejection holds for arbitrary input bytes — defending
// the invariant that decompress never silently *produces* output for
// factor=0 (which would corrupt the upstream caller's view of the file).
// -------------------------------------------------------------------------
proptest! {
    #[test]
    fn prop_decompress_factor_zero_returns_error(
        data in proptest::collection::vec(any::<u8>(), 0..512),
        rows in 1usize..=32,
        cols in 1usize..=32,
    ) {
        let dec = Decompressor::new();
        let result = dec.decompress(&data, 0, rows, cols);
        prop_assert!(
            result.is_err(),
            "compression_factor=0 must be rejected; got Ok({} floats)",
            result.as_ref().map(|v| v.len()).unwrap_or(0)
        );
    }
}

// -------------------------------------------------------------------------
// Pins A5 (revised after live-hardware verification): HTTP framing.
//
// Live SpectranV6 servers frame packets as `{json}\n\x1e<binary>` —
// LF terminating the JSON line, RS prefixing the binary — i.e. TWO
// separator bytes. The parser must also accept a lone RS and a lone LF
// (spec-conservative peers). One deliberate ambiguity: after a lone LF,
// a first binary byte of 0x1E is indistinguishable from the two-byte
// form and is interpreted as framing (real hardware always sends the
// RS), so the lone-LF case filters that byte out of the search space.
// -------------------------------------------------------------------------
proptest! {
    #[test]
    fn prop_http_framing_separators(
        re in -1.0e6_f32..1.0e6_f32,
        im in -1.0e6_f32..1.0e6_f32,
    ) {
        let re_bytes = re.to_le_bytes();

        let mut binary = Vec::with_capacity(8);
        binary.extend_from_slice(&re_bytes);
        binary.extend_from_slice(&im.to_le_bytes());

        // Real-hardware two-byte framing, plus each lone separator.
        let separators: [&[u8]; 3] = [b"\x0a\x1e", b"\x1e", b"\x0a"];
        for sep in separators {
            if sep == b"\x0a" && re_bytes[0] == 0x1E {
                // Lone-LF + leading-0x1E is the documented ambiguity;
                // the parser resolves it as two-byte framing.
                continue;
            }
            let mut parser = StreamParser::new(StreamFormat::Float32, None).unwrap();
            let packet = wrap_packet_sep(&binary, "iq", 1, sep);
            let result = parser.process_data(&packet)
                .unwrap_or_else(|e| panic!("sep={sep:02X?} parse failed: {e}"));
            prop_assert_eq!(
                result.len(), 1,
                "sep={:02X?} should produce one packet, got {}",
                sep, result.len()
            );
            prop_assert_eq!(result[0].samples.len(), 1);
            // f32 round-trip is exact for any finite f32.
            prop_assert!((result[0].samples[0].re - re).abs() < 1.0e-3);
            prop_assert!((result[0].samples[0].im - im).abs() < 1.0e-3);
        }
    }
}

// -------------------------------------------------------------------------
// Pins A3: IQ-mode `spanfreq * 1.5 <= receiverclock`.
//
// Hardware constraint for the Spectran V6 IQ-Mode receiver:
// `span_frequency * 1.5 ≤ receiver_clock`.
// Misconfigurations cause the SDK to silently emit corrupted samples,
// so we reject them at the API boundary. This property generates a
// `(clock, span_factor)` pair where `span = clock * span_factor`, then
// asserts `validate_iq_mode` accepts iff `span_factor <= 1/1.5`.
// -------------------------------------------------------------------------
proptest! {
    #[test]
    fn prop_validate_iq_mode_boundary(
        clock in 1.0e6_f64..1.0e9_f64,
        span_factor in 0.001_f64..2.0_f64,
    ) {
        let span = clock * span_factor;
        let result = validate_iq_mode(span, clock);
        let max_span = clock / 1.5;

        // Leave a hair of slack around the exact boundary for fp rounding.
        // Anything more than `1 + epsilon` over `max_span` must reject;
        // anything more than `1 - epsilon` under must accept. The narrow
        // band straddling `span == max_span` is acceptable either way.
        let eps = 1.0e-9 * max_span;
        if span <= max_span - eps {
            prop_assert!(
                result.is_ok(),
                "span={:.3e} clock={:.3e} (max={:.3e}) should be accepted; got {:?}",
                span, clock, max_span, result.err()
            );
        } else if span > max_span + eps {
            prop_assert!(
                result.is_err(),
                "span={:.3e} clock={:.3e} (max={:.3e}) should be rejected",
                span, clock, max_span
            );
        }
        // else: within fp slack of the boundary — either answer is fine.
    }
}

// -------------------------------------------------------------------------
// DSFH creation_time normalization.
//
// Some captures store DSFH::mCreationTime in microseconds since the
// Unix epoch rather than the documented seconds. The
// `rtsa_epoch_seconds` helper applies a value-range heuristic to
// normalise both forms. This property confirms the heuristic is a
// fixed point for plausible-Unix-seconds values AND maps the
// microseconds form back to the same seconds value.
// -------------------------------------------------------------------------
proptest! {
    #[test]
    fn prop_creation_time_normalization(
        // Plausible Unix timestamps: year 2001 (~1e9) through year 2065 (~3e9).
        seconds in 1.0e9_f64..3.0e9_f64,
    ) {
        // (1) The seconds form is a fixed point.
        let normalized_from_seconds = rtsa_epoch_seconds(seconds);
        prop_assert!(
            (normalized_from_seconds - seconds).abs() < 1.0e-3,
            "seconds form should be fixed point: rtsa_epoch_seconds({}) = {}",
            seconds, normalized_from_seconds
        );

        // (2) The microseconds form normalises to the same wall-clock.
        let microseconds = seconds * 1.0e6;
        let normalized_from_micros = rtsa_epoch_seconds(microseconds);
        // µs → s loses precision at the 1e-3 level for timestamps in this range.
        prop_assert!(
            (normalized_from_micros - seconds).abs() < 1.0,
            "microseconds form should normalise to seconds: rtsa_epoch_seconds({}) = {} (expected ~{})",
            microseconds, normalized_from_micros, seconds
        );
    }
}

// -------------------------------------------------------------------------
// Regression: RS-framed packets must not swallow separator-valued bytes.
//
// `raw_re = -4834` was the minimal failing case proptest discovered
// before the framing fix: its little-endian bytes are `[0x1E, 0xED]`,
// and the parser used to skip a *run* of separators — eating the
// leading `0x1E` as if it were continued framing. With RS framing the
// LF+RS double-skip never triggers (it requires an LF terminator), so
// a leading 0x1E must decode as data. Scale semantics: encode
// multiplier, decode divides (live-verified).
// -------------------------------------------------------------------------
#[test]
fn regression_int16_sample_starts_with_rs_byte() {
    let scale = 1.0e6_f64;
    let raw_re: i16 = -4834; // [0x1E, 0xED] little-endian
    let raw_im: i16 = 0;
    let mut parser = StreamParser::new(StreamFormat::Int16, Some(scale)).unwrap();
    let mut binary = Vec::with_capacity(4);
    binary.extend_from_slice(&raw_re.to_le_bytes());
    binary.extend_from_slice(&raw_im.to_le_bytes());
    let packet = wrap_packet(&binary, "iq", 1);
    let result = parser
        .process_data(&packet)
        .expect("parse should succeed even when binary starts with 0x1E");
    assert_eq!(
        result.len(),
        1,
        "RS framing must not swallow a leading 0x1E data byte"
    );
    let got_re = result[0].samples[0].re;
    let expected_re = (raw_re as f64 / scale) as f32;
    assert!(
        (got_re - expected_re).abs() < 1e-9,
        "decoded re={got_re}, expected {expected_re} (raw=-4834, scale=1e6)"
    );
}

/// Same regression but with `0x0A` (LF) as the byte that would have
/// been swallowed: `raw_re = 0x0A` (i16 = 10) directly puts `0x0A` as
/// the first byte of binary payload after RS framing.
#[test]
fn regression_int16_sample_starts_with_lf_byte() {
    let scale = 1.0e6_f64;
    let raw_re: i16 = 0x0A; // [0x0A, 0x00] little-endian
    let raw_im: i16 = 0;
    let mut parser = StreamParser::new(StreamFormat::Int16, Some(scale)).unwrap();
    let mut binary = Vec::with_capacity(4);
    binary.extend_from_slice(&raw_re.to_le_bytes());
    binary.extend_from_slice(&raw_im.to_le_bytes());
    let packet = wrap_packet(&binary, "iq", 1);
    let result = parser
        .process_data(&packet)
        .expect("parse should succeed even when binary starts with 0x0A");
    assert_eq!(
        result.len(),
        1,
        "RS framing must not swallow a leading 0x0A data byte"
    );
    let expected_re = (raw_re as f64 / scale) as f32;
    assert!(
        (result[0].samples[0].re - expected_re).abs() < 1e-9,
        "decoded sample differs from raw/scale"
    );
}

// -------------------------------------------------------------------------
// Survives-anything: the HTTP stream parser must not panic on arbitrary
// bytes. Migrated from `tests/integration_test.rs` so all proptest cases
// live together.
// -------------------------------------------------------------------------
proptest! {
    #[test]
    fn prop_stream_parser_does_not_panic(
        format_idx in 0usize..4,
        data in proptest::collection::vec(any::<u8>(), 0..2048),
    ) {
        let format = match format_idx {
            0 => StreamFormat::Json,
            1 => StreamFormat::Int16,
            2 => StreamFormat::Float16,
            _ => StreamFormat::Float32,
        };
        let mut parser = StreamParser::new(format, None).unwrap();
        // The parser must return Ok or Err for any input; never panic.
        let _ = parser.process_data(&Bytes::from(data));
    }
}
