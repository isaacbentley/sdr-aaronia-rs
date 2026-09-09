//! Aaronia RTSA Native SDK Integration
//!
//! This module provides a high-level wrapper over [`crate::native_sdk`].
//!
//! Unlike `native_sdk` itself, this module carries **no** internal
//! platform/feature fallback: `lib.rs` gates `pub mod sdk_source;` behind
//! the exact same condition (`feature = "native-sdk"` and
//! `target_os = "windows"` or `"linux"`) that `native_sdk` uses, so
//! everything below is only ever compiled when the native SDK is
//! actually available. Callers on unsupported platforms simply don't see
//! this module — there is no runtime fallback path to construct or call
//! (an earlier revision of this file *had* a `#[cfg(not(...))]` runtime
//! fallback, but that condition can never be true inside a module gated
//! identically at the crate root, so it was dead code and used before
//! this file compiled a `use anyhow::anyhow` import that no longer
//! exists in this crate).
//!
//! For HTTP-based streaming from an Aaronia RTSA Suite Pro instance, use
//! [`crate::SpectranSource`] (in [`crate::unified_source`]) instead.

use crate::Result;
use crate::utils::RxChannel;
use std::time::Duration;

pub mod native_sdk {
    // Re-export everything from the native_sdk module
    pub use crate::native_sdk::*;
}

/// High-level SDK source wrapper for easier integration with the native Spectran V6 SDK.
pub struct SdkSource {
    native_source: Option<crate::native_sdk::NativeSdkSource>,
    config: SdkConfig,
}

/// Configuration for SDK source
#[derive(Debug, Clone)]
pub struct SdkConfig {
    /// SDK device family (`"spectranv6"`, `"spectranv6eco"`), optionally
    /// mode-qualified (`"spectranv6/raw"`). Enumeration always uses the
    /// bare family — the SDK silently returns zero devices for
    /// mode-qualified enumeration — and opening uses the qualified form,
    /// defaulting to the family's IQ mode when none is given
    /// (`spectranv6/raw`; `spectranv6eco/iqreceiver` on the ECO — its
    /// `/rtsa` is the spectrum pipeline and its `/raw` ignores the
    /// requested rate).
    pub device_type: String,
    /// Center frequency in Hz.
    pub center_frequency_hz: f64,
    /// IQ span (sample rate) in Hz.
    pub sample_rate_hz: f64,
    /// Reference level in dBm.
    pub reference_level_dbm: f64,
    /// Device operation timeout.
    ///
    /// **Currently not applied.** Nothing in this wrapper or in
    /// [`crate::native_sdk`] reads this value — the only timeout actually
    /// in force on the read path is
    /// [`NativeSdkSource::READ_POLL_DEADLINE`](crate::native_sdk::NativeSdkSource::READ_POLL_DEADLINE),
    /// a fixed 500 ms poll deadline. Setting this field therefore has no
    /// effect on device behaviour today.
    ///
    /// It is kept (rather than removed) because the field is `pub` on a
    /// published crate, and honouring it would mean changing
    /// `NativeSdkSource::read_samples`' signature — both breaking changes.
    /// Documented here so callers don't set it expecting a behaviour change.
    pub timeout: Duration,
    /// Receiver channel selection, applied after the base IQ-receiver
    /// configuration during [`SdkSource::start_streaming`]. `None`
    /// keeps the default (`Rx1`). Only valid on `spectranv6/raw`;
    /// an explicit selection on another mode fails `start_streaming`.
    /// With [`RxChannel::Rx1And2`], read both channels via
    /// [`SdkSource::read_samples_dual`].
    pub receiver_channel: Option<RxChannel>,
}

impl Default for SdkConfig {
    fn default() -> Self {
        Self {
            // The SDK family string. The earlier default "Spectran_V6"
            // matched nothing: AARTSAAPI_EnumDevice expects "spectranv6".
            device_type: "spectranv6".to_string(),
            center_frequency_hz: 1e9,   // 1 GHz
            sample_rate_hz: 10e6,       // 10 MHz
            reference_level_dbm: -20.0, // -20 dBm
            timeout: Duration::from_secs(30),
            receiver_channel: None,
        }
    }
}

impl SdkConfig {
    /// Bare device family for `AARTSAAPI_EnumDevice` (strips any `/mode`).
    pub fn device_family(&self) -> &str {
        crate::native_sdk::split_device_type(&self.device_type, "raw").0
    }

    /// Mode-qualified open string for `AARTSAAPI_OpenDevice`. Uses the
    /// configured mode when present, otherwise the family's IQ mode:
    /// `spectranv6/raw`, or `spectranv6eco/iqreceiver` on the ECO. The
    /// ECO's `/raw` does open, but carries no `main/spanfreq`, so it
    /// cannot honour a requested sample rate; `/rtsa` is its spectrum
    /// pipeline and yields IQ at a fraction of a megasample.
    pub fn device_open_mode(&self) -> String {
        let family = self.device_family();
        let raw_mode = crate::native_sdk::raw_mode_for_family(family);
        let open_mode = crate::native_sdk::split_device_type(&self.device_type, raw_mode).1;
        // An explicit `/raw` on the ECO is the same request, spelled the
        // V6 way; map it as `open_detected_device` does.
        if open_mode == "spectranv6eco/raw" {
            "spectranv6eco/iqreceiver".to_string()
        } else {
            open_mode
        }
    }
}

impl SdkSource {
    /// Create a new SDK source with default configuration
    pub fn new() -> Self {
        Self {
            native_source: None,
            config: SdkConfig::default(),
        }
    }

    /// Create a new SDK source with custom configuration
    pub fn with_config(config: SdkConfig) -> Self {
        Self {
            native_source: None,
            config,
        }
    }

    /// Initialize the SDK source
    pub async fn initialize(&mut self) -> Result<()> {
        let mut native_source = unsafe { crate::native_sdk::NativeSdkSource::new()? };
        unsafe { native_source.initialize()? };
        self.native_source = Some(native_source);
        Ok(())
    }

    /// Check if the source is available
    pub fn is_available(&self) -> bool {
        self.native_source.is_some()
    }

    /// Get the current configuration
    pub fn get_config(&self) -> &SdkConfig {
        &self.config
    }

    /// Update configuration
    pub fn update_config(&mut self, config: SdkConfig) {
        self.config = config;
    }

    /// Start streaming from the SDK source
    pub async fn start_streaming(&mut self) -> Result<()> {
        let native_source = self
            .native_source
            .as_mut()
            .ok_or_else(|| crate::Error::Sdk("Source not initialized".to_string()))?;

        unsafe {
            // Enumerate with the bare family; open with the
            // mode-qualified string. Passing a mode-qualified
            // string to enumeration makes the SDK silently
            // return zero devices.
            let family = self.config.device_family().to_string();
            let open_mode = self.config.device_open_mode();
            let devices = native_source.find_devices(&family)?;
            if devices.is_empty() {
                return Err(crate::Error::Sdk(format!("No {} devices found", family)));
            }

            let device_info = &devices[0];
            if !device_info.ready() {
                return Err(crate::Error::Sdk("Device not ready".to_string()));
            }

            native_source.open_device(&open_mode, &device_info.serial_number)?;

            // Configure device. The channel selection is a parameter of
            // configure_iq_receiver (not a follow-up call) so any future
            // reconfiguration path re-applies it automatically.
            native_source.configure_iq_receiver(
                self.config.center_frequency_hz,
                self.config.sample_rate_hz,
                self.config.reference_level_dbm,
                self.config.receiver_channel,
            )?;

            // Start streaming
            native_source.start_streaming()?;
        }
        Ok(())
    }

    /// Stop streaming
    pub async fn stop_streaming(&mut self) -> Result<()> {
        let native_source = self
            .native_source
            .as_mut()
            .ok_or_else(|| crate::Error::Sdk("Source not initialized".to_string()))?;
        unsafe { native_source.stop_streaming()? };
        Ok(())
    }

    /// Read samples from the source
    pub async fn read_samples(
        &mut self,
        buffer: &mut Vec<num_complex::Complex32>,
        max_samples: usize,
    ) -> Result<usize> {
        match self.native_source.as_mut() {
            Some(native_source) => Ok(unsafe { native_source.read_samples(buffer, max_samples)? }),
            None => Ok(0),
        }
    }

    /// Read up to `max_samples` time-aligned sample *pairs* from a
    /// dual-channel ([`RxChannel::Rx1And2`]) stream — Rx1 into `rx1`,
    /// Rx2 into `rx2`. Returns the number of pairs appended. See
    /// [`NativeSdkSource::read_samples_dual`](crate::native_sdk::NativeSdkSource::read_samples_dual)
    /// for the packet-layout contract and its hardware-verification
    /// caveat; don't mix this with [`Self::read_samples`] on one stream.
    pub async fn read_samples_dual(
        &mut self,
        rx1: &mut Vec<num_complex::Complex32>,
        rx2: &mut Vec<num_complex::Complex32>,
        max_samples: usize,
    ) -> Result<usize> {
        match self.native_source.as_mut() {
            Some(native_source) => {
                Ok(unsafe { native_source.read_samples_dual(rx1, rx2, max_samples)? })
            }
            None => Ok(0),
        }
    }

    /// Check if streaming is active
    pub fn is_streaming(&self) -> bool {
        self.native_source
            .as_ref()
            .map(|s| s.is_streaming())
            .unwrap_or(false)
    }
}

impl Default for SdkSource {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_sdk_config_default() {
        let config = SdkConfig::default();
        assert_eq!(config.device_type, "spectranv6");
        assert_eq!(config.center_frequency_hz, 1e9);
        assert_eq!(config.sample_rate_hz, 10e6);
        assert_eq!(config.reference_level_dbm, -20.0);
        assert_eq!(config.timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_sdk_config_creation() {
        let config = SdkConfig {
            device_type: "Test_Device".to_string(),
            center_frequency_hz: 2.4e9,
            sample_rate_hz: 20e6,
            reference_level_dbm: -30.0,
            timeout: Duration::from_secs(60),
            receiver_channel: None,
        };

        assert_eq!(config.device_type, "Test_Device");
        assert_eq!(config.center_frequency_hz, 2.4e9);
        assert_eq!(config.sample_rate_hz, 20e6);
        assert_eq!(config.reference_level_dbm, -30.0);
        assert_eq!(config.timeout, Duration::from_secs(60));
    }

    #[test]
    fn test_sdk_config_clone() {
        let config = SdkConfig::default();
        let cloned = config.clone();

        assert_eq!(config.device_type, cloned.device_type);
        assert_eq!(config.center_frequency_hz, cloned.center_frequency_hz);
        assert_eq!(config.sample_rate_hz, cloned.sample_rate_hz);
        assert_eq!(config.reference_level_dbm, cloned.reference_level_dbm);
        assert_eq!(config.timeout, cloned.timeout);
    }

    #[test]
    fn test_sdk_config_debug() {
        let config = SdkConfig::default();
        let debug_str = format!("{:?}", config);

        assert!(debug_str.contains("SdkConfig"));
        assert!(debug_str.contains("device_type"));
        assert!(debug_str.contains("center_frequency_hz"));
        assert!(debug_str.contains("sample_rate_hz"));
        assert!(debug_str.contains("reference_level_dbm"));
        assert!(debug_str.contains("timeout"));
    }

    #[test]
    fn test_sdk_source_new() {
        let source = SdkSource::new();
        assert!(!source.is_available()); // Should not be available until initialized
        assert!(!source.is_streaming());

        let config = source.get_config();
        assert_eq!(config.device_type, "spectranv6");
    }

    #[test]
    fn test_sdk_source_with_config() {
        let config = SdkConfig {
            device_type: "Custom_Device".to_string(),
            center_frequency_hz: 5.8e9,
            sample_rate_hz: 40e6,
            reference_level_dbm: -10.0,
            timeout: Duration::from_secs(15),
            receiver_channel: Some(RxChannel::Rx2),
        };

        let source = SdkSource::with_config(config.clone());
        let source_config = source.get_config();

        assert_eq!(source_config.device_type, config.device_type);
        assert_eq!(
            source_config.center_frequency_hz,
            config.center_frequency_hz
        );
        assert_eq!(source_config.sample_rate_hz, config.sample_rate_hz);
        assert_eq!(
            source_config.reference_level_dbm,
            config.reference_level_dbm
        );
        assert_eq!(source_config.timeout, config.timeout);
        assert_eq!(source_config.receiver_channel, Some(RxChannel::Rx2));
    }

    #[test]
    fn test_sdk_source_default() {
        let source1 = SdkSource::new();
        let source2 = SdkSource::default();

        assert_eq!(
            source1.get_config().device_type,
            source2.get_config().device_type
        );
        assert_eq!(
            source1.get_config().center_frequency_hz,
            source2.get_config().center_frequency_hz
        );
    }

    #[test]
    fn test_sdk_source_config_update() {
        let mut source = SdkSource::new();

        let new_config = SdkConfig {
            device_type: "Updated_Device".to_string(),
            center_frequency_hz: 3.5e9,
            sample_rate_hz: 50e6,
            reference_level_dbm: -15.0,
            timeout: Duration::from_secs(45),
            receiver_channel: Some(RxChannel::Rx1And2),
        };

        source.update_config(new_config.clone());
        let updated_config = source.get_config();

        assert_eq!(updated_config.device_type, new_config.device_type);
        assert_eq!(
            updated_config.center_frequency_hz,
            new_config.center_frequency_hz
        );
        assert_eq!(updated_config.sample_rate_hz, new_config.sample_rate_hz);
        assert_eq!(
            updated_config.reference_level_dbm,
            new_config.reference_level_dbm
        );
        assert_eq!(updated_config.timeout, new_config.timeout);
        assert_eq!(updated_config.receiver_channel, Some(RxChannel::Rx1And2));
    }

    #[test]
    fn test_sdk_source_initial_state() {
        let source = SdkSource::new();

        // Initially not available and not streaming
        assert!(!source.is_available());
        assert!(!source.is_streaming());
    }

    /// Poll a future to completion with a no-op waker, with no async
    /// runtime dependency. `sdk_source` is gated to `native-sdk` +
    /// windows/linux and does *not* require the `http` feature, so its
    /// tests can't assume `tokio` is in the dependency graph — `#[tokio::
    /// test]` here previously broke `cargo test --no-default-features
    /// --features native-sdk` (no `http`, hence no `tokio`) the moment
    /// this module's tests actually ran on a supported OS (a case the
    /// crate's macOS-only local dev loop can't exercise, since this
    /// whole module compiles away there). Sound here specifically
    /// because `SdkSource::read_samples`'s body never `.await`s
    /// anything that can return `Pending` — it's a synchronous FFI call
    /// wrapped in `async fn` purely to match the rest of the crate's
    /// async surface — so the first `poll` always resolves.
    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        let mut fut = Box::pin(fut);
        let waker = std::task::Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);
        match fut.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(val) => val,
            std::task::Poll::Pending => panic!(
                "read_samples() unexpectedly returned Pending; it must complete synchronously"
            ),
        }
    }

    #[test]
    fn test_sdk_source_read_samples_empty() {
        let mut source = SdkSource::new();

        // Without initialization, should return 0 samples
        let mut buffer = Vec::new();
        let n = block_on(source.read_samples(&mut buffer, 1024)).unwrap();
        assert_eq!(n, 0);
        assert!(buffer.is_empty());
    }

    /// The ECO has no `/raw`; its raw pipeline is `rtsa`.
    #[test]
    fn bare_family_opens_in_its_raw_mode() {
        let mut config = SdkConfig {
            device_type: "spectranv6".to_string(),
            ..Default::default()
        };
        assert_eq!(config.device_open_mode(), "spectranv6/raw");
        config.device_type = "spectranv6eco".to_string();
        assert_eq!(config.device_family(), "spectranv6eco");
        assert_eq!(config.device_open_mode(), "spectranv6eco/iqreceiver");
        // `/raw` spelled the V6 way maps to the ECO's name for it.
        config.device_type = "spectranv6eco/raw".to_string();
        assert_eq!(config.device_open_mode(), "spectranv6eco/iqreceiver");
        // Any other explicit mode is passed through untouched.
        config.device_type = "spectranv6eco/iqreceiver".to_string();
        assert_eq!(config.device_open_mode(), "spectranv6eco/iqreceiver");
        // A trailing slash names no mode, so it is the bare family again.
        // This used to yield `"spectranv6eco/"`: the empty mode segment
        // counted as "already qualified" and so never met the remap above.
        config.device_type = "spectranv6eco/".to_string();
        assert_eq!(config.device_family(), "spectranv6eco");
        assert_eq!(config.device_open_mode(), "spectranv6eco/iqreceiver");
        config.device_type = "spectranv6/".to_string();
        assert_eq!(config.device_open_mode(), "spectranv6/raw");
    }

    /// `raw_mode_for_family` is a two-way branch: everything that is not
    /// the ECO gets `/raw`, including a family this build has never heard
    /// of. Worth pinning because the fallback is what a future device
    /// family will hit before anyone teaches the crate about it.
    ///
    /// The family name here is deliberately one Aaronia will never ship.
    /// A plausible-looking `spectranv6mk2` would quietly stop testing the
    /// fallback the day someone added support for it.
    #[test]
    fn unknown_family_falls_back_to_raw() {
        let mut config = SdkConfig {
            device_type: "not_a_real_family".to_string(),
            ..Default::default()
        };
        assert_eq!(config.device_family(), "not_a_real_family");
        assert_eq!(config.device_open_mode(), "not_a_real_family/raw");

        // An explicit mode on an unknown family is passed through rather
        // than rewritten — only `spectranv6eco/raw` is remapped.
        config.device_type = "not_a_real_family/sweepsa".to_string();
        assert_eq!(config.device_family(), "not_a_real_family");
        assert_eq!(config.device_open_mode(), "not_a_real_family/sweepsa");
    }
}
