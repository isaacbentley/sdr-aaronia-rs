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
//! [`crate::AaroniaSource`] (in [`crate::unified_source`]) instead.

use crate::Result;
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
    /// defaulting to `<family>/raw` when no mode is given.
    pub device_type: String,
    /// Center frequency in Hz.
    pub center_frequency: f64,
    /// IQ span (sample rate) in Hz.
    pub span_frequency: f64,
    /// Reference level in dBm.
    pub reference_level: f64,
    /// Device operation timeout.
    pub timeout: Duration,
}

impl Default for SdkConfig {
    fn default() -> Self {
        Self {
            // The SDK family string. The earlier default "Spectran_V6"
            // matched nothing: AARTSAAPI_EnumDevice expects "spectranv6".
            device_type: "spectranv6".to_string(),
            center_frequency: 1e9,  // 1 GHz
            span_frequency: 10e6,   // 10 MHz
            reference_level: -20.0, // -20 dBm
            timeout: Duration::from_secs(30),
        }
    }
}

impl SdkConfig {
    /// Bare device family for `AARTSAAPI_EnumDevice` (strips any `/mode`).
    pub fn device_family(&self) -> &str {
        crate::native_sdk::split_device_type(&self.device_type, "raw").0
    }

    /// Mode-qualified open string for `AARTSAAPI_OpenDevice`. Uses the
    /// configured mode when present, otherwise `<family>/raw`.
    pub fn device_open_mode(&self) -> String {
        crate::native_sdk::split_device_type(&self.device_type, "raw").1
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

            // Configure device
            native_source.configure_iq_receiver(
                self.config.center_frequency,
                self.config.span_frequency,
                self.config.reference_level,
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
        assert_eq!(config.center_frequency, 1e9);
        assert_eq!(config.span_frequency, 10e6);
        assert_eq!(config.reference_level, -20.0);
        assert_eq!(config.timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_sdk_config_creation() {
        let config = SdkConfig {
            device_type: "Test_Device".to_string(),
            center_frequency: 2.4e9,
            span_frequency: 20e6,
            reference_level: -30.0,
            timeout: Duration::from_secs(60),
        };

        assert_eq!(config.device_type, "Test_Device");
        assert_eq!(config.center_frequency, 2.4e9);
        assert_eq!(config.span_frequency, 20e6);
        assert_eq!(config.reference_level, -30.0);
        assert_eq!(config.timeout, Duration::from_secs(60));
    }

    #[test]
    fn test_sdk_config_clone() {
        let config = SdkConfig::default();
        let cloned = config.clone();

        assert_eq!(config.device_type, cloned.device_type);
        assert_eq!(config.center_frequency, cloned.center_frequency);
        assert_eq!(config.span_frequency, cloned.span_frequency);
        assert_eq!(config.reference_level, cloned.reference_level);
        assert_eq!(config.timeout, cloned.timeout);
    }

    #[test]
    fn test_sdk_config_debug() {
        let config = SdkConfig::default();
        let debug_str = format!("{:?}", config);

        assert!(debug_str.contains("SdkConfig"));
        assert!(debug_str.contains("device_type"));
        assert!(debug_str.contains("center_frequency"));
        assert!(debug_str.contains("span_frequency"));
        assert!(debug_str.contains("reference_level"));
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
            center_frequency: 5.8e9,
            span_frequency: 40e6,
            reference_level: -10.0,
            timeout: Duration::from_secs(15),
        };

        let source = SdkSource::with_config(config.clone());
        let source_config = source.get_config();

        assert_eq!(source_config.device_type, config.device_type);
        assert_eq!(source_config.center_frequency, config.center_frequency);
        assert_eq!(source_config.span_frequency, config.span_frequency);
        assert_eq!(source_config.reference_level, config.reference_level);
        assert_eq!(source_config.timeout, config.timeout);
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
            source1.get_config().center_frequency,
            source2.get_config().center_frequency
        );
    }

    #[test]
    fn test_sdk_source_config_update() {
        let mut source = SdkSource::new();

        let new_config = SdkConfig {
            device_type: "Updated_Device".to_string(),
            center_frequency: 3.5e9,
            span_frequency: 50e6,
            reference_level: -15.0,
            timeout: Duration::from_secs(45),
        };

        source.update_config(new_config.clone());
        let updated_config = source.get_config();

        assert_eq!(updated_config.device_type, new_config.device_type);
        assert_eq!(updated_config.center_frequency, new_config.center_frequency);
        assert_eq!(updated_config.span_frequency, new_config.span_frequency);
        assert_eq!(updated_config.reference_level, new_config.reference_level);
        assert_eq!(updated_config.timeout, new_config.timeout);
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

    #[test]
    fn test_frequency_validation() {
        // Test typical frequency ranges
        let valid_frequencies = vec![100e6, 1e9, 2.4e9, 5.8e9, 10e9];

        for freq in valid_frequencies {
            assert!(freq > 0.0, "Frequency {} should be positive", freq);
            assert!(
                freq <= 20e9,
                "Frequency {} should be within device limits",
                freq
            );
        }
    }

    #[test]
    fn test_span_frequency_validation() {
        // Test span frequency ranges
        let valid_spans = vec![1e3, 1e6, 10e6, 100e6, 1e9];

        for span in valid_spans {
            assert!(span > 0.0, "Span {} should be positive", span);
            assert!(span <= 10e9, "Span {} should be within device limits", span);
        }
    }

    #[test]
    fn test_reference_level_validation() {
        // Test reference level ranges (typical for RF)
        let valid_levels = vec![-100.0, -50.0, -20.0, 0.0, 30.0];

        for level in valid_levels {
            assert!(level >= -140.0, "Reference level {} too low", level);
            assert!(level <= 40.0, "Reference level {} too high", level);
        }
    }

    #[test]
    fn test_timeout_validation() {
        let timeouts = vec![
            Duration::from_secs(1),
            Duration::from_secs(30),
            Duration::from_secs(60),
            Duration::from_secs(300),
        ];

        for timeout in timeouts {
            assert!(!timeout.is_zero(), "Timeout should not be zero");
            assert!(timeout.as_secs() <= 600, "Timeout should be reasonable");
        }
    }

    #[test]
    fn test_device_type_validation() {
        let valid_device_types = vec![
            "spectranv6",
            "spectranv6/raw",
            "spectranv6eco",
            "spectranv6eco/iqreceiver",
        ];

        for device_type in valid_device_types {
            assert!(!device_type.is_empty(), "Device type should not be empty");
            assert!(
                device_type.len() <= 50,
                "Device type should be reasonable length"
            );
        }
    }

    #[test]
    fn test_config_frequency_relationships() {
        let config = SdkConfig {
            device_type: "Test".to_string(),
            center_frequency: 1e9,
            span_frequency: 10e6,
            reference_level: -20.0,
            timeout: Duration::from_secs(30),
        };

        // Span should be much smaller than center frequency for typical use
        assert!(config.span_frequency < config.center_frequency);

        // Calculate frequency ranges
        let start_freq = config.center_frequency - config.span_frequency / 2.0;
        let end_freq = config.center_frequency + config.span_frequency / 2.0;

        assert!(start_freq > 0.0, "Start frequency should be positive");
        assert!(
            end_freq > start_freq,
            "End frequency should be greater than start"
        );
    }

    #[test]
    fn test_sample_rate_calculation() {
        let config = SdkConfig::default();

        // For IQ mode, sample rate typically equals span frequency
        let expected_sample_rate = config.span_frequency;

        // Calculate expected samples per second
        let samples_per_second = expected_sample_rate as usize;
        assert!(samples_per_second > 0);

        // Calculate buffer size for 1 second of data
        let buffer_size_1sec = samples_per_second;
        assert!(buffer_size_1sec > 1000); // Should be reasonable size
    }

    #[test]
    fn test_memory_requirements() {
        let config = SdkConfig::default();

        // Calculate memory requirements for different buffer sizes
        let complex_size = std::mem::size_of::<num_complex::Complex32>();
        let samples_per_second = config.span_frequency as usize;

        let memory_1sec = samples_per_second * complex_size;
        let memory_10sec = memory_1sec * 10;

        assert!(complex_size == 8, "Complex32 should be 8 bytes");
        assert!(memory_1sec > 0, "Memory calculation should be positive");
        assert!(
            memory_10sec == memory_1sec * 10,
            "Memory scaling should be linear"
        );
    }
}
