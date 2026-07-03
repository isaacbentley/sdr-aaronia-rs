//! Utility functions for working with Aaronia devices and RF data
//!
//! This module provides common helper functions that make it easier to work
//! with Aaronia SPECTRAN V6 devices and RF data processing.

use crate::{Error, Result};

/// HTTP `User-Agent` string used by every outbound request from this
/// crate (both the lightweight endpoints client and the streaming
/// source).
///
/// Picks the first non-empty value of:
///
/// 1. The `AARONIA_USER_AGENT` environment variable (caller-supplied
///    branding — e.g. `MyApp/2.1.0`).
/// 2. The default `sdr-aaronia-rs/<crate version>`.
///
/// This replaces previous hard-coded strings, which leaked a downstream
/// application name into a library that's now used outside that
/// project.
pub fn user_agent() -> String {
    match std::env::var("AARONIA_USER_AGENT") {
        Ok(s) if !s.trim().is_empty() => s,
        _ => format!("sdr-aaronia-rs/{}", env!("CARGO_PKG_VERSION")),
    }
}

/// Parse frequency strings with units (e.g., "146.52M", "2.4G", "162.5k")
///
/// Supports common RF units:
/// - Hz (no suffix)
/// - kHz (k/K suffix)
/// - MHz (m/M suffix)
/// - GHz (g/G suffix)
///
/// # Examples
/// ```
/// use sdr_aaronia_rs::utils::parse_frequency;
///
/// assert_eq!(parse_frequency("146.52M").unwrap(), 146_520_000.0);
/// assert_eq!(parse_frequency("2.4G").unwrap(), 2_400_000_000.0);
/// assert_eq!(parse_frequency("162.5k").unwrap(), 162_500.0);
/// assert_eq!(parse_frequency("100000").unwrap(), 100_000.0);
/// ```
pub fn parse_frequency(s: &str) -> Result<f64> {
    let s = s.trim();
    let (value_str, unit_str) = if s.ends_with("k") || s.ends_with("K") {
        (&s[0..s.len() - 1], "k")
    } else if s.ends_with("m") || s.ends_with("M") {
        (&s[0..s.len() - 1], "M")
    } else if s.ends_with("g") || s.ends_with("G") {
        (&s[0..s.len() - 1], "G")
    } else {
        (s, "")
    };

    let value = value_str
        .parse::<f64>()
        .map_err(|_| Error::Config(format!("Invalid frequency value: {}", value_str)))?;

    match unit_str {
        "k" => Ok(value * 1e3),
        "M" => Ok(value * 1e6),
        "G" => Ok(value * 1e9),
        _ => Ok(value), // Assume Hz if no unit specified
    }
}

/// Parse sample rate strings with units (e.g., "25M", "10k", "2.5M")
///
/// Same as [`parse_frequency`] but provides better semantic naming for sample rates.
///
/// # Examples
/// ```
/// use sdr_aaronia_rs::utils::parse_sample_rate;
///
/// assert_eq!(parse_sample_rate("25M").unwrap(), 25_000_000.0);
/// assert_eq!(parse_sample_rate("10k").unwrap(), 10_000.0);
/// ```
pub fn parse_sample_rate(s: &str) -> Result<f64> {
    parse_frequency(s)
}

/// Convert linear power ratio to dB
///
/// # Examples
/// ```
/// use sdr_aaronia_rs::utils::linear_to_db;
///
/// assert_eq!(linear_to_db(1.0), 0.0);
/// assert!((linear_to_db(10.0) - 10.0).abs() < 1e-10);
/// assert!((linear_to_db(0.1) - (-10.0)).abs() < 1e-10);
/// ```
pub fn linear_to_db(linear: f64) -> f64 {
    if linear <= 0.0 {
        return f64::NEG_INFINITY;
    }
    10.0 * linear.log10()
}

/// Convert dB to linear power ratio
///
/// # Examples
/// ```
/// use sdr_aaronia_rs::utils::db_to_linear;
///
/// assert_eq!(db_to_linear(0.0), 1.0);
/// assert!((db_to_linear(10.0) - 10.0).abs() < 1e-10);
/// assert!((db_to_linear(-10.0) - 0.1).abs() < 1e-10);
/// ```
pub fn db_to_linear(db: f64) -> f64 {
    10.0_f64.powf(db / 10.0)
}

/// Format frequency for human display
///
/// Automatically chooses appropriate units and precision.
///
/// # Examples
/// ```
/// use sdr_aaronia_rs::utils::format_frequency;
///
/// assert_eq!(format_frequency(146_520_000.0), "146.52 MHz");
/// assert_eq!(format_frequency(2_400_000_000.0), "2.40 GHz");
/// assert_eq!(format_frequency(162_500.0), "162.50 kHz");
/// ```
pub fn format_frequency(freq_hz: f64) -> String {
    if freq_hz >= 1e9 {
        format!("{:.2} GHz", freq_hz / 1e9)
    } else if freq_hz >= 1e6 {
        format!("{:.2} MHz", freq_hz / 1e6)
    } else if freq_hz >= 1e3 {
        format!("{:.2} kHz", freq_hz / 1e3)
    } else {
        format!("{:.1} Hz", freq_hz)
    }
}

/// Format sample rate for human display
///
/// # Examples
/// ```
/// use sdr_aaronia_rs::utils::format_sample_rate;
///
/// assert_eq!(format_sample_rate(25_000_000.0), "25.0 Msps");
/// assert_eq!(format_sample_rate(10_000.0), "10.0 ksps");
/// ```
pub fn format_sample_rate(rate_hz: f64) -> String {
    if rate_hz >= 1e6 {
        format!("{:.1} Msps", rate_hz / 1e6)
    } else if rate_hz >= 1e3 {
        format!("{:.1} ksps", rate_hz / 1e3)
    } else {
        format!("{:.1} sps", rate_hz)
    }
}

/// Default IQ Mode receiver clock for the SpectranV6 family, used when the
/// device hasn't been opened yet. `configure_iq_receiver` writes this same
/// value to `device/receiverclock` at configuration time, so it matches the
/// SDK runtime once a session is established.
///
/// The ConfigItem strings the SDK exposes (`"46MHz"`, `"92MHz"`, etc.) are
/// rounded labels; the actual receiver clock for `"92MHz"` is **92.16 MHz**
/// per the official RTSA-API-Samples README. Use [`receiver_clock_for_label`]
/// to resolve any of the documented labels to its physical rate.
pub const DEFAULT_RECEIVER_CLOCK_HZ: f64 = 92_160_000.0;

/// Resolve a `device/receiverclock` ConfigItem label (e.g. `"92MHz"`) to
/// its actual physical rate in Hz. The mapping comes from the official
/// RTSA-API-Samples README. Unknown labels fall back to the integer-MHz
/// parse so we degrade gracefully on future firmware additions.
pub fn receiver_clock_for_label(label: &str) -> f64 {
    match label.trim() {
        // Documented in the README "ConfigItem ↔ ActualRate" table.
        "46MHz" => 46_080_000.0,
        "61MHz" => 61_440_000.0,
        "76MHz" | "77MHz" => 76_800_000.0, // both labels map to the same rate
        "92MHz" => 92_160_000.0,
        "122MHz" => 122_880_000.0,
        "184MHz" => 184_320_000.0, // not in the README table but in the enum
        "245MHz" => 245_760_000.0,
        "492MHz" => 491_520_000.0, // ditto
        other => {
            // Fallback: parse the leading integer and multiply by 1e6. Loses
            // the ".xx MHz" precision, but keeps `validate_iq_mode` working
            // on labels we haven't enumerated.
            other
                .trim_end_matches("MHz")
                .trim()
                .parse::<f64>()
                .map(|mhz| mhz * 1e6)
                .unwrap_or(DEFAULT_RECEIVER_CLOCK_HZ)
        }
    }
}

/// Hardware constraint for IQ Mode: the configured span frequency
/// must satisfy `span_freq * 1.5 ≤ receiver_clock`. Misconfigurations cause
/// the SDK to silently emit corrupted samples; reject them at the API
/// boundary instead.
pub fn validate_iq_mode(span_freq_hz: f64, receiver_clock_hz: f64) -> Result<()> {
    if !span_freq_hz.is_finite() || span_freq_hz <= 0.0 {
        return Err(Error::Config(format!(
            "span_frequency must be a finite positive number (got {})",
            span_freq_hz
        )));
    }
    if !receiver_clock_hz.is_finite() || receiver_clock_hz <= 0.0 {
        return Err(Error::Config(format!(
            "receiver_clock must be a finite positive number (got {})",
            receiver_clock_hz
        )));
    }
    let max_span = receiver_clock_hz / 1.5;
    if span_freq_hz > max_span {
        return Err(Error::Config(format!(
            "IQ Mode constraint violated: span_frequency {:.3} MHz exceeds \
             receiver_clock / 1.5 = {:.3} MHz. Lower the span \
             or raise the receiver clock.",
            span_freq_hz / 1e6,
            max_span / 1e6
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frequency() {
        assert_eq!(parse_frequency("146.52M").unwrap(), 146_520_000.0);
        assert_eq!(parse_frequency("2.4G").unwrap(), 2_400_000_000.0);
        assert_eq!(parse_frequency("162.5k").unwrap(), 162_500.0);
        assert_eq!(parse_frequency("100000").unwrap(), 100_000.0);
    }

    #[test]
    fn test_db_conversions() {
        assert_eq!(linear_to_db(1.0), 0.0);
        assert!((linear_to_db(10.0) - 10.0).abs() < 1e-10);
        assert_eq!(db_to_linear(0.0), 1.0);
        assert!((db_to_linear(10.0) - 10.0).abs() < 1e-10);
    }

    #[test]
    fn test_format_frequency() {
        assert_eq!(format_frequency(146_520_000.0), "146.52 MHz");
        assert_eq!(format_frequency(2_400_000_000.0), "2.40 GHz");
        assert_eq!(format_frequency(162_500.0), "162.50 kHz");
    }

    /// Process-wide env-var manipulation makes this test inherently
    /// non-parallel-safe; the parallel test suite mutates `AARONIA_USER_AGENT`
    /// in a single thread by setting → reading → unsetting in sequence,
    /// using a `#[serial]`-equivalent restore-on-drop guard. Acceptable
    /// because the helper is small and the test is dirt cheap.
    #[test]
    fn test_user_agent_default_and_override() {
        // Restore the original env value on drop so we don't bleed
        // state into other tests.
        struct Guard {
            original: Option<String>,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                match self.original.take() {
                    Some(v) => unsafe { std::env::set_var("AARONIA_USER_AGENT", v) },
                    None => unsafe { std::env::remove_var("AARONIA_USER_AGENT") },
                }
            }
        }
        let _g = Guard {
            original: std::env::var("AARONIA_USER_AGENT").ok(),
        };

        // Default: no env var set → "sdr-aaronia-rs/<version>".
        unsafe { std::env::remove_var("AARONIA_USER_AGENT") };
        let default_ua = user_agent();
        assert!(
            default_ua.starts_with("sdr-aaronia-rs/"),
            "default UA should start with crate name; got {default_ua}"
        );

        // Override via env var.
        unsafe { std::env::set_var("AARONIA_USER_AGENT", "MyApp/1.2.3") };
        assert_eq!(user_agent(), "MyApp/1.2.3");

        // Empty env var falls back to default (not the empty string).
        unsafe { std::env::set_var("AARONIA_USER_AGENT", "   ") };
        assert!(user_agent().starts_with("sdr-aaronia-rs/"));
    }

    #[test]
    fn validate_iq_mode_accepts_at_boundary() {
        // span * 1.5 == clock; ≤ is allowed.
        validate_iq_mode(DEFAULT_RECEIVER_CLOCK_HZ / 1.5, DEFAULT_RECEIVER_CLOCK_HZ)
            .expect("boundary case should validate");
    }

    #[test]
    fn validate_iq_mode_rejects_above_boundary() {
        // span just above the limit must be rejected.
        let just_over = DEFAULT_RECEIVER_CLOCK_HZ / 1.5 * 1.000_001;
        let err = validate_iq_mode(just_over, DEFAULT_RECEIVER_CLOCK_HZ)
            .expect_err("expected violation when span > clock / 1.5");
        let msg = err.to_string();
        assert!(
            msg.contains("IQ Mode constraint"),
            "error should reference IQ Mode constraint, got: {msg}"
        );
    }

    #[test]
    fn validate_iq_mode_rejects_zero_or_negative() {
        assert!(validate_iq_mode(0.0, DEFAULT_RECEIVER_CLOCK_HZ).is_err());
        assert!(validate_iq_mode(-1.0, DEFAULT_RECEIVER_CLOCK_HZ).is_err());
        assert!(validate_iq_mode(15.36e6, 0.0).is_err());
    }

    #[test]
    fn validate_iq_mode_rejects_non_finite() {
        assert!(validate_iq_mode(f64::NAN, DEFAULT_RECEIVER_CLOCK_HZ).is_err());
        assert!(validate_iq_mode(f64::INFINITY, DEFAULT_RECEIVER_CLOCK_HZ).is_err());
        assert!(validate_iq_mode(15.36e6, f64::INFINITY).is_err());
    }

    #[test]
    fn validate_iq_mode_passes_dji_default() {
        // 15.36 MHz IQ span must work on a default 92.16 MHz clock.
        validate_iq_mode(15.36e6, DEFAULT_RECEIVER_CLOCK_HZ)
            .expect("DJI native span must be valid against the default clock");
    }

    #[test]
    fn receiver_clock_for_label_documented_values() {
        // Each entry is taken straight from the official RTSA-API-Samples
        // README ConfigItem ↔ ActualRate table.
        let cases = [
            ("46MHz", 46_080_000.0),
            ("61MHz", 61_440_000.0),
            ("76MHz", 76_800_000.0),
            ("77MHz", 76_800_000.0), // alias
            ("92MHz", 92_160_000.0),
            ("122MHz", 122_880_000.0),
            ("184MHz", 184_320_000.0),
            ("245MHz", 245_760_000.0),
            ("492MHz", 491_520_000.0),
        ];
        for (label, expected) in cases {
            assert_eq!(
                receiver_clock_for_label(label),
                expected,
                "label {} should map to {} Hz",
                label,
                expected
            );
        }
    }

    #[test]
    fn receiver_clock_for_label_unknown_falls_back_to_integer_parse() {
        // A label we haven't enumerated still produces a sensible rate so
        // future SDK revisions don't immediately break validation.
        assert_eq!(receiver_clock_for_label("310MHz"), 310e6);
        // Garbage input falls back to the documented default clock so the
        // IQ-Mode constraint is checked against a known-safe value.
        assert_eq!(
            receiver_clock_for_label("whatever"),
            DEFAULT_RECEIVER_CLOCK_HZ
        );
    }

    #[test]
    fn default_receiver_clock_matches_92mhz_label() {
        // The default constant must agree with the label resolver to avoid
        // drift between code paths.
        assert_eq!(DEFAULT_RECEIVER_CLOCK_HZ, receiver_clock_for_label("92MHz"));
    }
}
