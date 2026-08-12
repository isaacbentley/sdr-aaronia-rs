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

/// Which physical RF input(s) a Spectran V6 capture uses.
///
/// Maps to the native SDK's `device/receiverchannel` config item
/// (`NativeSdkSource::set_receiver_channel`, `native-sdk` feature,
/// Windows/Linux). Defined here — always compiled — so cross-platform
/// configuration code can name a channel without feature/target gates;
/// only *applying* it requires the native SDK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RxChannel {
    /// First RF input (default).
    Rx1,
    /// Second RF input (full V6 only; hardware-unverified).
    Rx2,
    /// Both inputs interleaved into a single stream (full V6 only;
    /// hardware-unverified). Read with `read_samples_dual` on the
    /// native SDK source.
    ///
    /// The device offers two ways to run both inputs and they are not
    /// interchangeable. `"Rx12"`, used here, interleaves the pair into
    /// one stream: four floats per sample, `[I0, Q0, I1, Q1]`, read
    /// from stream 0. `"Rx1+Rx2"` instead delivers two independent
    /// streams that must be fetched and consumed separately, at indices
    /// 0 and 1. Aaronia's `RawIQ2RXInterleave` and `RawIQ2RX` samples
    /// show one each. This crate reads a single stream and
    /// deinterleaves it, so it requires the former.
    Rx1And2,
}

impl RxChannel {
    /// The exact string the SDK config item expects.
    pub fn as_config_str(&self) -> &'static str {
        match self {
            Self::Rx1 => "Rx1",
            Self::Rx2 => "Rx2",
            // Not "Rx1+Rx2": that selects two separate streams, while
            // this crate reads one interleaved stream. See the variant
            // documentation above.
            Self::Rx1And2 => "Rx12",
        }
    }
}

/// Deinterleave one dual-channel IQ packet into per-channel sample pairs.
///
/// In `Rx1+Rx2` capture the SDK delivers both receivers in a single
/// packet: each sample occupies `stride` floats, laid out
/// `[I1, Q1, I2, Q2, ...pad]` (channel 1 in floats 0–1, channel 2 in
/// floats 2–3; any remaining floats up to `stride` are padding). On
/// success this yields `(rx1, rx2)` pairs in sample order — as a
/// borrowed iterator, so the per-packet hot path pays no intermediate
/// allocation.
///
/// `floats` must hold at least `(num_samples - 1) * stride + 4` values —
/// the last sample's four channel floats — and `stride` must be ≥ 4;
/// anything less cannot be a dual-channel layout, and callers should
/// treat it as "the stream is not dual-channel" (e.g.
/// `device/receiverchannel` was not set to `Rx1+Rx2`). All bounds are
/// validated up front, so the iterator itself never panics.
pub fn deinterleave_dual_iq<'a>(
    floats: &'a [f32],
    num_samples: usize,
    stride: usize,
) -> Result<impl ExactSizeIterator<Item = (num_complex::Complex32, num_complex::Complex32)> + 'a> {
    if stride < 4 {
        return Err(Error::Sdk(format!(
            "packet stride {} cannot carry two interleaved IQ channels (need >= 4); \
             is device/receiverchannel set to Rx12?",
            stride
        )));
    }
    if num_samples > 0 {
        let needed = (num_samples - 1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(4))
            .ok_or_else(|| {
                Error::Sdk(format!(
                    "dual-channel packet dimensions overflow: {} samples x stride {}",
                    num_samples, stride
                ))
            })?;
        if floats.len() < needed {
            return Err(Error::Sdk(format!(
                "dual-channel packet too short: {} floats for {} samples at stride {} (need {})",
                floats.len(),
                num_samples,
                stride,
                needed
            )));
        }
    }
    Ok((0..num_samples).map(move |i| {
        let base = i * stride;
        (
            num_complex::Complex32::new(floats[base], floats[base + 1]),
            num_complex::Complex32::new(floats[base + 2], floats[base + 3]),
        )
    }))
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

/// Highest IQ sample rate available with the default receiver clock.
///
/// This is the [`DEFAULT_RECEIVER_CLOCK_HZ`] divided by the 1.5 factor
/// that [`validate_iq_mode`] enforces, and it matches the maximum
/// measured on a SPECTRAN V6 ECO. A full V6 can select a faster
/// receiver clock, so use [`iq_sample_rates_for_clock`] when the clock
/// is known rather than assuming this ceiling.
pub const IQ_CLOCK_HZ: f64 = 61_440_000.0;

/// Fraction of the sample rate that the device declares as usable RF
/// bandwidth.
///
/// Every sample reaches the caller, so an FFT of them spans the whole
/// rate; this is the part of it that the anti-alias filter keeps flat
/// and that calibration covers. RTSA reports it as
/// `startFrequency..endFrequency`, and the ratio is exactly 0.8 at
/// every rate — a fixed rule rather than a per-rate measurement.
/// Checked at 61.44, 15.36, 7.68 and 3.84 MHz on a SPECTRAN V6 ECO,
/// all reading 0.8000.
///
/// Measuring the receiver's own noise floor on that device backs the
/// figure up: the response is flat to within 0.5 dB across 0.80 of the
/// rate at 15.36 MHz sampling and 0.89 at 7.68 MHz. Full span is the
/// tight case — the analog filter is roughly 1 dB down by the declared
/// edge and 3 dB down at 0.84 of the rate — which is why Aaronia's
/// data sheet quotes 44 MHz for the ECO rather than the 49.152 MHz it
/// declares there.
pub const USABLE_BANDWIDTH_RATIO: f64 = 0.8;

/// The IQ sample rates available at the default receiver clock,
/// highest first.
///
/// The device divides [`IQ_CLOCK_HZ`] by powers of two, which the RTSA
/// GUI shows as Full through `1 / 512`. Nothing between these exists:
/// a request for any other rate is adjusted, so a caller that assumes
/// it got what it asked for will compute every derived frequency
/// wrongly.
///
/// This is a V6 ECO's ladder, measured rung by rung. A full V6 selects
/// its receiver clock and starts higher — pass that clock to
/// [`iq_sample_rates_for_clock`], and read its caveat first.
pub fn iq_sample_rates() -> [f64; 10] {
    iq_sample_rates_for_clock(DEFAULT_RECEIVER_CLOCK_HZ)
}

/// The IQ sample rates available at a given receiver clock, highest
/// first.
///
/// The top rate is `receiver_clock_hz / 1.5`, the most that
/// [`validate_iq_mode`] permits, and each step halves it.
///
/// **Measured only at the default clock.** A V6 ECO has a fixed
/// receiver clock and produced exactly the ladder this returns for
/// [`DEFAULT_RECEIVER_CLOCK_HZ`], verified rung by rung. A full V6 can
/// select other clocks — Aaronia's own samples use `"92MHz"` and
/// `"245MHz"` — and the rates there follow the same constraint but have
/// not been confirmed against hardware.
///
/// **The 1.5 may be wrong for a full V6.** Aaronia advertise 245 MHz of
/// real-time bandwidth per input and "the full 250M samples of IQ
/// data", which is the `245MHz` clock label itself rather than the
/// 163.84 MHz this returns for it. That may simply mean the fastest
/// rate needs the `492MHz` clock, where 245.76 MHz of span does
/// satisfy the 1.5 rule; a published Remote Config panel showing a
/// full V6 at a `92MHz` clock with a 92.16 MHz native rate fits
/// neither reading. Until one can be measured, treat this as the ECO's
/// ladder and prefer the rate a device reports in its stream metadata
/// over the one computed here.
pub fn iq_sample_rates_for_clock(receiver_clock_hz: f64) -> [f64; 10] {
    let top = receiver_clock_hz / 1.5;
    let mut rates = [0.0; 10];
    for (n, rate) in rates.iter_mut().enumerate() {
        *rate = top / f64::from(1u32 << n);
    }
    rates
}

/// Alias-free bandwidth delivered at `sample_rate_hz`.
///
/// This is the width the stream's `startFrequency`..`endFrequency`
/// covers, and it is smaller than the sample rate. Use it when asking
/// "how much spectrum am I actually seeing", and the sample rate itself
/// when converting sample indices or FFT bins to time or frequency.
pub fn usable_bandwidth_hz(sample_rate_hz: f64) -> f64 {
    sample_rate_hz * USABLE_BANDWIDTH_RATIO
}

/// The supported sample rate closest to `requested_hz`.
///
/// Use this to report an achievable rate before streaming starts. Note
/// that the RTSA server, given an unsupported rate, does not choose the
/// nearest one: see [`iq_sample_rate_for_bandwidth`].
pub fn nearest_iq_sample_rate(requested_hz: f64) -> f64 {
    iq_sample_rates()
        .into_iter()
        .min_by(|a, b| {
            (a - requested_hz)
                .abs()
                .total_cmp(&(b - requested_hz).abs())
        })
        .unwrap_or(IQ_CLOCK_HZ)
}

/// The lowest sample rate whose alias-free bandwidth covers
/// `bandwidth_hz`.
///
/// This is the calculation to use when a caller thinks in terms of "I
/// need to see N Hz of spectrum". Wanting 8 MHz of usable bandwidth
/// needs 10 MHz of sampling, and the lowest rate that provides it is
/// 15.36 MHz. Multiplying the desired bandwidth by some factor and
/// hoping is how callers end up processing at a rate the hardware is
/// not running: the device would honour such a request by picking its
/// own rate, leaving every derived frequency wrong.
///
/// Returns [`IQ_CLOCK_HZ`] when the request exceeds what the hardware
/// can cover.
pub fn iq_sample_rate_for_bandwidth(bandwidth_hz: f64) -> f64 {
    let needed = bandwidth_hz / USABLE_BANDWIDTH_RATIO;
    iq_sample_rates()
        .into_iter()
        .filter(|rate| *rate >= needed)
        .fold(None::<f64>, |best, rate| {
            Some(match best {
                Some(b) if b <= rate => b,
                _ => rate,
            })
        })
        .unwrap_or(IQ_CLOCK_HZ)
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
mod ladder_tests {
    use super::*;

    /// The ten rates the device reports in its own decimation enum.
    /// A V6 with a faster receiver clock reaches higher rates. Aaronia's
    /// samples select "245MHz", which the constraint puts at 163.84 MHz
    /// of span. Inferred from the constraint, not measured.
    #[test]
    fn faster_clock_raises_the_ceiling() {
        let fast = iq_sample_rates_for_clock(245_760_000.0);
        assert!((fast[0] - 163_840_000.0).abs() < 1.0);
        assert!(
            fast[0] > iq_sample_rates()[0],
            "a faster clock must reach further"
        );
        for pair in fast.windows(2) {
            assert!((pair[0] / pair[1] - 2.0).abs() < 1e-9);
        }
    }

    #[test]
    fn ladder_matches_the_device() {
        let rates = iq_sample_rates();
        assert_eq!(rates[0], 61_440_000.0, "Full");
        assert_eq!(rates[2], 15_360_000.0, "1 / 4");
        assert_eq!(rates[9], 120_000.0, "1 / 512");
        for pair in rates.windows(2) {
            assert!((pair[0] / pair[1] - 2.0).abs() < 1e-9, "each step halves");
        }
    }

    /// Measured on hardware at four separate rates.
    #[test]
    fn usable_bandwidth_is_four_fifths_of_the_rate() {
        assert_eq!(usable_bandwidth_hz(15_360_000.0), 12_288_000.0);
        assert_eq!(usable_bandwidth_hz(30_720_000.0), 24_576_000.0);
        assert_eq!(usable_bandwidth_hz(7_680_000.0), 6_144_000.0);
        assert_eq!(usable_bandwidth_hz(120_000.0), 96_000.0);
    }

    #[test]
    fn nearest_rate_snaps_to_the_ladder() {
        assert_eq!(nearest_iq_sample_rate(15_360_000.0), 15_360_000.0);
        // 10 MHz sits between 7.68 and 15.36, closer to 7.68.
        assert_eq!(nearest_iq_sample_rate(10_000_000.0), 7_680_000.0);
        assert_eq!(nearest_iq_sample_rate(1.0e9), 61_440_000.0);
        assert_eq!(nearest_iq_sample_rate(1.0), 120_000.0);
    }

    /// Wanting N Hz of spectrum needs N / 0.8 of sampling, rounded up to
    /// a rate that exists.
    #[test]
    fn rate_for_bandwidth_covers_the_request() {
        // 8 MHz of spectrum needs 10 MHz of sampling; 7.68 is too slow.
        assert_eq!(iq_sample_rate_for_bandwidth(8_000_000.0), 15_360_000.0);
        // An exact fit is not rounded up unnecessarily.
        assert_eq!(iq_sample_rate_for_bandwidth(12_288_000.0), 15_360_000.0);
        assert_eq!(iq_sample_rate_for_bandwidth(6_144_000.0), 7_680_000.0);
        // Beyond the hardware, report the maximum rather than nothing.
        assert_eq!(iq_sample_rate_for_bandwidth(1.0e9), 61_440_000.0);

        for want in [50_000.0, 1.0e6, 5.0e6, 20.0e6, 49.0e6] {
            let rate = iq_sample_rate_for_bandwidth(want);
            assert!(
                usable_bandwidth_hz(rate) >= want || rate == IQ_CLOCK_HZ,
                "{want} Hz should be covered by {rate} Hz sampling"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rx_channel_config_strings() {
        // Exact strings the SDK's device/receiverchannel item expects,
        // per the official RTSA-API-Samples.
        assert_eq!(RxChannel::Rx1.as_config_str(), "Rx1");
        assert_eq!(RxChannel::Rx2.as_config_str(), "Rx2");
        // "Rx12" interleaves both inputs into one stream, which is
        // what this crate reads. "Rx1+Rx2" would deliver two separate
        // streams and silently break the deinterleave.
        assert_eq!(RxChannel::Rx1And2.as_config_str(), "Rx12");
    }

    #[test]
    fn test_deinterleave_dual_iq_tight_stride() {
        // Two samples, tightly packed at stride 4: [I1 Q1 I2 Q2] each.
        let floats = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let pairs: Vec<_> = deinterleave_dual_iq(&floats, 2, 4).unwrap().collect();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, num_complex::Complex32::new(1.0, 2.0));
        assert_eq!(pairs[0].1, num_complex::Complex32::new(3.0, 4.0));
        assert_eq!(pairs[1].0, num_complex::Complex32::new(5.0, 6.0));
        assert_eq!(pairs[1].1, num_complex::Complex32::new(7.0, 8.0));
    }

    #[test]
    fn test_deinterleave_dual_iq_padded_stride() {
        // Stride 6 with two padding floats per sample; the pad values
        // (99.0) must never appear in either channel. The buffer ends
        // at the last sample's fourth float — no trailing pad — which
        // also pins the `(num-1)*stride + 4` length rule.
        let floats = [
            1.0, 2.0, 3.0, 4.0, 99.0, 99.0, // sample 0 + pad
            5.0, 6.0, 7.0, 8.0, // sample 1, no trailing pad
        ];
        let pairs: Vec<_> = deinterleave_dual_iq(&floats, 2, 6).unwrap().collect();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[1].0, num_complex::Complex32::new(5.0, 6.0));
        assert_eq!(pairs[1].1, num_complex::Complex32::new(7.0, 8.0));
    }

    #[test]
    fn test_deinterleave_dual_iq_rejects_narrow_stride() {
        // Stride 2 is a single-channel layout: the caller almost
        // certainly forgot to set device/receiverchannel to Rx1+Rx2.
        // (`match` instead of `unwrap_err`: the Ok type is an opaque
        // iterator without Debug.)
        let floats = [1.0, 2.0, 3.0, 4.0];
        let err = match deinterleave_dual_iq(&floats, 2, 2) {
            Err(e) => e,
            Ok(_) => panic!("stride 2 must be rejected"),
        };
        assert!(err.to_string().contains("Rx12"), "got: {err}");
    }

    #[test]
    fn test_deinterleave_dual_iq_rejects_short_buffer() {
        // Claims 3 samples at stride 4 but only carries 2.
        let floats = [0.0; 8];
        assert!(deinterleave_dual_iq(&floats, 3, 4).is_err());
    }

    #[test]
    fn test_deinterleave_dual_iq_empty() {
        assert_eq!(deinterleave_dual_iq(&[], 0, 4).unwrap().count(), 0);
    }

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
