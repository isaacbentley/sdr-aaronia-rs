//! Device-provided limits, shared by HTTP and native SDK callers.

/// A numeric setting's declared bounds, as the device states them.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "http", derive(serde::Serialize))]
pub struct ValueRange {
    pub min: f64,
    pub max: f64,
    /// Distance between valid values, when the device declares one.
    pub step: Option<f64>,
}

/// What the device says about itself: its identity and the bounds it
/// declares on the settings a client can drive.
///
/// Supplied by HTTP `/remoteconfig` and `/healthstatus`, or directly by
/// the opened SDK device's ConfigInfo. These are configuration bounds,
/// not hardware qualification or an amplitude calibration certificate.
/// SDK bounds can exceed the installed model's rated bandwidth.
///
/// Every field is optional and independently so: a tree that does not
/// carry an item leaves its field `None`, and a caller falls back for
/// that field alone rather than discarding a whole reading.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "http", derive(serde::Serialize))]
#[non_exhaustive]
pub struct DeviceCapabilities {
    /// Transport that supplied this reading: `native-sdk` or `http`.
    pub transport: Option<String>,
    /// Selected SDK family/mode; absent for HTTP.
    pub device_mode: Option<String>,
    /// SDK API version; this is not a firmware version.
    pub sdk_version: Option<u32>,
    /// SDK-declared IQ receiver span setting bounds. These may exceed the
    /// hardware's rated RF bandwidth; use packet span for acquired samples.
    pub rf_span_hz: Option<ValueRange>,
    /// Receiver clock options, including options disabled by hardware/licensing.
    pub receiver_clocks: Vec<ConfigOption>,
    /// Clock currently selected by the SDK.
    pub receiver_clock: Option<String>,
    /// Receiver channel options, including disabled ones.
    pub receiver_channels: Vec<ConfigOption>,
    /// Raw decimation options; index n selects factor 2^n.
    pub decimations: Vec<ConfigOption>,
    /// Human-readable model, e.g. `"SPECTRAN V6 ECO"` — `/healthstatus`
    /// `info/devname`, else `/remoteconfig` `info/title`.
    pub model: Option<String>,
    /// `/healthstatus` `info/serialno`.
    pub serial: Option<String>,
    /// `/healthstatus` `info/version` — firmware/FPGA revisions.
    pub version: Option<String>,
    /// Bounds on `centerfreq0`, in Hz.
    pub center_frequency_hz: Option<ValueRange>,
    /// Bounds on `reflevel0`, in dBm.
    pub reference_level_dbm: Option<ValueRange>,
    /// How many rungs `decimation0` offers — `"Full,1 / 2,…"` counted,
    /// so a device with a shorter ladder is not advertised ten.
    pub decimation_steps: Option<usize>,
    /// `/healthstatus` `status/iqsamples`: the **native**, undecimated
    /// IQ rate, which is not the rate the stream is running at. A
    /// measurement, so see [`Self::sample_rates`] before using it as a
    /// ladder top.
    pub native_iq_rate_hz: Option<f64>,
    /// Every stream-clock source `device/sclksource` offers, in the
    /// device's own vocabulary — `Consumer`, `Oscillator`, `GPS`,
    /// `PPS`, `10MHz` and the three `… Provider` variants on a measured
    /// V6 ECO. Empty when the item is absent.
    pub clock_sources: Vec<String>,
    /// The source `sclksource` currently selects. A device running off
    /// a house 10 MHz reference says `10MHz` here, which is worth
    /// reporting accurately: an operator who wired that reference up
    /// for frequency accuracy needs to see it took.
    pub clock_source: Option<String>,
    /// Every GPS mode `device/gpsmode` offers that the device will
    /// currently accept — `Disabled`, `Location`, `Time`, `Location and
    /// Time` on a measured V6 ECO. This is what governs whether GPS
    /// disciplines the clock and whether `gps_time_ns` ever reports a
    /// fix; the device ships on `Disabled`, in which state GPS time
    /// never arrives and the API looks broken. Empty when the item is
    /// absent.
    pub gps_modes: Vec<String>,
    /// The GPS mode `gpsmode` currently selects.
    pub gps_mode: Option<String>,
    /// The RX input the device's `devicemode` names — `"RX1"` or
    /// `"RX2"`. Read-only on a V6 ECO, which reports `RX1 LO1 SWEEP`.
    /// `None` when the mode names no RX input (a TX-only mode) or the
    /// item is absent.
    pub rx_antenna: Option<String>,
}

/// One SDK enum option. Position is retained because disabled-options bits
/// and ConfigSetInteger refer to the original position, not a filtered list.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "http", derive(serde::Serialize))]
pub struct ConfigOption {
    pub index: usize,
    pub label: String,
    pub enabled: bool,
}

/// Split SDK ConfigInfo's semicolon-separated options without renumbering them.
#[cfg(any(
    test,
    all(
        feature = "native-sdk",
        any(target_os = "windows", target_os = "linux")
    )
))]
pub(crate) fn sdk_options(labels: &str, disabled: u64) -> Vec<ConfigOption> {
    labels
        .split(';')
        .enumerate()
        .filter_map(|(index, label)| {
            let label = label.trim();
            if label.is_empty() {
                return None;
            }
            Some(ConfigOption {
                index,
                label: label.to_owned(),
                enabled: index >= 64 || disabled & (1u64 << index) == 0,
            })
        })
        .collect()
}

/// Explicit receive-only native SDK configuration. Unset fields preserve the
/// legacy defaults; setting RF span does not reinterpret the IQ sample-rate API.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NativeReceiverOptions {
    /// Bare SDK family. Enables explicit discovery of future family variants.
    pub family: Option<String>,
    /// Mode suffix (`iqreceiver` or `raw`). No transmit mode is enabled here.
    pub mode: Option<String>,
    /// RF span in Hz for an IQ receiver mode, validated against ConfigInfo.
    pub rf_span_hz: Option<f64>,
    /// SDK receiver clock enum label for raw mode (e.g. `245MHz`).
    pub receiver_clock: Option<String>,
    /// Raw decimation factor (power of two); validated against device options.
    pub decimation_factor: Option<u32>,
}

impl ValueRange {
    /// Reject malformed metadata rather than claiming invented device limits.
    pub fn new(min: f64, max: f64, step: f64) -> Option<Self> {
        (min.is_finite() && max.is_finite() && min <= max).then_some(Self {
            min,
            max,
            step: (step.is_finite() && step > 0.0).then_some(step),
        })
    }
    pub fn contains(&self, value: f64) -> bool {
        value.is_finite() && (self.min..=self.max).contains(&value)
    }
}

impl DeviceCapabilities {
    /// Versioned capability JSON for bindings. Unknown fields serialize as null.
    #[cfg(feature = "http")]
    pub fn to_json(&self) -> crate::Result<String> {
        serde_json::to_string(&serde_json::json!({"schema_version": 1, "capabilities": self}))
            .map_err(|error| crate::Error::Protocol(error.to_string()))
    }

    /// The sample rates this device can actually be set to, highest
    /// first, or `None` when the reading does not support an answer.
    ///
    /// Both halves have to come from the device for this to be worth
    /// more than the compiled-in ladder: the top rung from
    /// [`Self::native_iq_rate_hz`] — snapped to an exact rung, because
    /// the reported figure is a measurement — and the depth from
    /// [`Self::decimation_steps`], so a device offering fewer rungs is
    /// not advertised more. Missing or unrecognised either way gives
    /// `None`, which is the caller's signal to fall back rather than
    /// publish a ladder the hardware will not honour.
    pub fn sample_rates(&self) -> Option<Vec<f64>> {
        let top = crate::utils::snap_to_ladder_top(self.native_iq_rate_hz?)?;
        let steps = self.decimation_steps?;
        if steps == 0 {
            return None;
        }
        Some(crate::utils::iq_ladder_from_top_n(top, steps))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_sdk_options_keep_their_indices() {
        let options = sdk_options("92MHz;;245MHz;492MHz;", 1 << 2);
        assert_eq!(
            options.iter().map(|o| o.index).collect::<Vec<_>>(),
            vec![0, 2, 3]
        );
        assert!(options[0].enabled);
        assert!(!options[1].enabled);
        assert!(options[2].enabled);
    }
    #[test]
    fn malformed_bounds_are_unknown() {
        assert!(ValueRange::new(f64::NAN, 44e6, 1.0).is_none());
        assert!(ValueRange::new(60e6, 44e6, 1.0).is_none());
        assert!(!ValueRange::new(0.0, 60e6, 0.0).unwrap().contains(f64::NAN));
    }
}
