//! Geometry declared by an IQ packet, independent of model and transport speed.

/// One IQ packet's frequency mapping. A valid RF span is not a sample rate,
/// and does not by itself assert calibrated amplitude accuracy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IqGeometry {
    pub sample_rate_hz: f64,
    pub bandwidth_hz: Option<f64>,
    pub center_frequency_hz: Option<f64>,
}

impl IqGeometry {
    /// Interpret the SDK's `startFrequency`, `stepFrequency` and
    /// `spanFrequency`. The header defines centre as start + span / 2.
    /// Unknown or malformed fields stay unknown instead of being inferred
    /// from a model-specific ratio. A negative start is valid near zero Hz.
    pub fn from_packet(start: f64, rate: f64, span: f64) -> Option<Self> {
        if !rate.is_finite() || rate <= 0.0 {
            return None;
        }
        let bandwidth_hz = (span.is_finite() && span > 0.0 && span <= rate).then_some(span);
        let center_frequency_hz = bandwidth_hz.and_then(|width| {
            let center = start + width / 2.0;
            (center.is_finite() && center >= 0.0).then_some(center)
        });
        Some(Self {
            sample_rate_hz: rate,
            bandwidth_hz,
            center_frequency_hz,
        })
    }
}

#[cfg(any(
    test,
    all(
        feature = "native-sdk",
        any(target_os = "windows", target_os = "linux")
    )
))]
pub(crate) fn validate_eco_span(span: f64, minimum: f64, maximum: f64) -> crate::Result<()> {
    if !(span.is_finite()
        && span > 0.0
        && maximum.is_finite()
        && maximum > 0.0
        && minimum.is_finite()
        && minimum >= 0.0
        && minimum <= span
        && span <= maximum)
    {
        return Err(crate::Error::Config(format!(
            "ECO IQ span {span} Hz is outside the device's {minimum}..{maximum} Hz RF bandwidth range"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observed_usb_rate_does_not_replace_rf_width_or_shift_the_center() {
        let g = IqGeometry::from_packet(259.52e6, 59_213_703.5294118, 40.96e6).unwrap();
        assert_eq!(g.bandwidth_hz, Some(40.96e6));
        assert_eq!(g.center_frequency_hz, Some(280e6));
        assert_ne!(g.sample_rate_hz, 61.44e6);
    }

    #[test]
    fn geometry_is_not_limited_to_one_spectran_model() {
        // Software fixtures for distinct bandwidths, not hardware certification.
        for (span, rate) in [
            (44e6, 59.2e6),
            (60e6, 90e6),
            (80e6, 122.88e6),
            (120e6, 184.32e6),
            (245e6, 368.64e6),
            (490e6, 737.28e6),
        ] {
            let g = IqGeometry::from_packet(1e9 - span / 2.0, rate, span).unwrap();
            assert_eq!(g.bandwidth_hz, Some(span));
            assert_eq!(g.center_frequency_hz, Some(1e9));
        }
        assert_eq!(
            IqGeometry::from_packet(-22e6 + 9e3, 66e6, 44e6)
                .unwrap()
                .center_frequency_hz,
            Some(9e3)
        );
    }

    #[test]
    fn invalid_geometry_remains_unknown() {
        for rate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(IqGeometry::from_packet(0.0, rate, 44e6).is_none());
        }
        for span in [0.0, -1.0, 70e6, f64::NAN, f64::INFINITY] {
            let g = IqGeometry::from_packet(1e9, 66e6, span).unwrap();
            assert_eq!(g.bandwidth_hz, None);
            assert_eq!(g.center_frequency_hz, None);
        }
        assert_eq!(
            IqGeometry::from_packet(f64::NAN, 66e6, 44e6)
                .unwrap()
                .center_frequency_hz,
            None
        );
    }

    #[test]
    fn eco_bounds_come_from_the_device() {
        assert!(validate_eco_span(44e6, 1e3, 44e6).is_ok());
        assert!(validate_eco_span(60e6, 1e3, 60e6).is_ok());
        assert!(validate_eco_span(45e6, 1e3, 44e6).is_err());
        for (min, max) in [
            (0.0, 0.0),
            (60e6, 44e6),
            (f64::NAN, 60e6),
            (0.0, f64::INFINITY),
        ] {
            assert!(validate_eco_span(44e6, min, max).is_err());
        }
    }
}
