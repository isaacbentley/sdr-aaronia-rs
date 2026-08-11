//! Unified transmission sink over the native SDK TX path.
//!
//! Wraps [`crate::sdk_sink::SdkSink`] behind a platform-uniform type so
//! FFI consumers (the SoapySDR plugin, C callers) can hold one sink
//! type everywhere. Transmission itself requires the Aaronia native SDK
//! and therefore the `native-sdk` feature on Windows or Linux; on every
//! other configuration [`UnifiedSink::initialize`] returns a clear
//! error instead of pretending success (an earlier revision reported
//! success from a stub and then failed on the first write, making
//! "unsupported platform" indistinguishable from a transient fault).
//!
//! > [!WARNING]
//! > The whole TX path is **hardware-unverified**: it drives
//! > `AARTSAAPI_SendPacket` per the vendor samples, but the development
//! > device (a single-channel V6 ECO) has not been used to confirm RF
//! > output. See [`crate::sdk_sink`] for the underlying caveats.

use crate::{Error, Result};
use num_complex::Complex32;

#[cfg(all(
    feature = "native-sdk",
    any(target_os = "windows", target_os = "linux")
))]
use crate::sdk_sink::{SdkSink, SdkSinkConfig};

/// Cross-platform sink configuration. Mirrors the tunable subset of
/// [`crate::sdk_sink::SdkSinkConfig`] without being feature/OS-gated,
/// so builders compile identically everywhere.
#[derive(Debug, Clone)]
pub struct UnifiedSinkConfig {
    /// SDK device family, optionally mode-qualified
    /// (default `"spectranv6"`, opened as `<family>/iqtransmitter`).
    pub device_type: String,
    /// TX center frequency in Hz.
    pub center_frequency: f64,
    /// IQ sample rate (span) in Hz.
    pub span_frequency: f64,
    /// Transmission gain in dB (typically 0.0 to -120.0).
    pub trans_gain: f64,
}

impl Default for UnifiedSinkConfig {
    fn default() -> Self {
        Self {
            device_type: "spectranv6".to_string(),
            center_frequency: 1.0e9,
            span_frequency: 10.0e6,
            trans_gain: -20.0,
        }
    }
}

/// A unified transmission sink. See the module docs for platform
/// availability and the hardware-unverified caveat.
pub struct UnifiedSink {
    config: UnifiedSinkConfig,
    #[cfg(all(
        feature = "native-sdk",
        any(target_os = "windows", target_os = "linux")
    ))]
    backend: Option<SdkSink>,
}

impl UnifiedSink {
    pub fn new() -> Self {
        Self::with_config(UnifiedSinkConfig::default())
    }

    /// The sink's current configuration.
    pub fn config(&self) -> &UnifiedSinkConfig {
        &self.config
    }

    pub fn with_config(config: UnifiedSinkConfig) -> Self {
        Self {
            config,
            #[cfg(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            ))]
            backend: None,
        }
    }

    /// Load the native SDK library. Does not touch the device;
    /// [`Self::start_streaming`] opens, configures, and starts it.
    ///
    /// Errors immediately on platforms without native-SDK TX support so
    /// callers learn at setup time, not first-write time.
    pub async fn initialize(&mut self) -> Result<()> {
        #[cfg(all(
            feature = "native-sdk",
            any(target_os = "windows", target_os = "linux")
        ))]
        {
            let mut sdk_sink = SdkSink::with_config(self.sdk_config());
            sdk_sink.initialize().await?;
            self.backend = Some(sdk_sink);
            Ok(())
        }
        #[cfg(not(all(
            feature = "native-sdk",
            any(target_os = "windows", target_os = "linux")
        )))]
        {
            Err(Error::Sdk(
                "transmission requires the Aaronia native SDK (feature \
                 `native-sdk` on Windows or Linux); this build has no TX \
                 backend"
                    .to_string(),
            ))
        }
    }

    /// Open the first matching device, configure the IQ transmitter
    /// from this sink's config, and start the TX stream. Must be called
    /// after [`Self::initialize`] and before [`Self::write_samples`] —
    /// an earlier revision omitted this entirely, leaving the FFI TX
    /// path pointed at a never-opened device.
    pub async fn start_streaming(&mut self) -> Result<()> {
        #[cfg(all(
            feature = "native-sdk",
            any(target_os = "windows", target_os = "linux")
        ))]
        {
            let backend = self
                .backend
                .as_mut()
                .ok_or_else(|| Error::Sdk("Sink not initialized".to_string()))?;
            backend.start_streaming().await
        }
        #[cfg(not(all(
            feature = "native-sdk",
            any(target_os = "windows", target_os = "linux")
        )))]
        {
            Err(Error::Sdk("no TX backend in this build".to_string()))
        }
    }

    pub async fn stop_streaming(&mut self) -> Result<()> {
        #[cfg(all(
            feature = "native-sdk",
            any(target_os = "windows", target_os = "linux")
        ))]
        if let Some(ref mut sdk_sink) = self.backend {
            sdk_sink.stop_streaming().await?;
        }
        Ok(())
    }

    /// Read the device's master stream clock, in seconds. TX burst
    /// times are expressed against this clock — not wall-clock time.
    pub fn get_master_stream_time(&mut self) -> Result<f64> {
        #[cfg(all(
            feature = "native-sdk",
            any(target_os = "windows", target_os = "linux")
        ))]
        {
            let backend = self
                .backend
                .as_mut()
                .ok_or_else(|| Error::Sdk("Sink not initialized".to_string()))?;
            backend.get_master_stream_time()
        }
        #[cfg(not(all(
            feature = "native-sdk",
            any(target_os = "windows", target_os = "linux")
        )))]
        {
            Err(Error::Sdk("no TX backend in this build".to_string()))
        }
    }

    /// Queue one burst of IQ samples for transmission.
    ///
    /// `start_time_s`/`end_time_s` are in **master stream time**
    /// seconds (see [`Self::get_master_stream_time`]); `flags` are
    /// [`crate::native_sdk::tx_flags`]-style packet boundary flags
    /// (callers streaming continuously should not set
    /// `SEGMENT_START|SEGMENT_END` on every packet — that was a
    /// hard-coded bug in an earlier revision that made multi-packet
    /// bursts inexpressible).
    #[cfg_attr(
        not(all(
            feature = "native-sdk",
            any(target_os = "windows", target_os = "linux")
        )),
        allow(unused_variables)
    )]
    pub fn write_samples(
        &mut self,
        channel: i32,
        start_time_s: f64,
        end_time_s: f64,
        flags: u64,
        samples: &[Complex32],
    ) -> Result<()> {
        #[cfg(all(
            feature = "native-sdk",
            any(target_os = "windows", target_os = "linux")
        ))]
        {
            let config_center = self.config.center_frequency;
            let config_rate = self.config.span_frequency;
            let backend = self
                .backend
                .as_mut()
                .ok_or_else(|| Error::Sdk("Sink not initialized".to_string()))?;
            let burst = crate::native_sdk::TxBurst {
                start_time: start_time_s,
                end_time: end_time_s,
                center_frequency_hz: config_center,
                sample_rate_hz: config_rate,
                flags,
            };
            backend.write_samples(channel, burst, samples)
        }
        #[cfg(not(all(
            feature = "native-sdk",
            any(target_os = "windows", target_os = "linux")
        )))]
        {
            Err(Error::Sdk("no TX backend in this build".to_string()))
        }
    }

    #[cfg(all(
        feature = "native-sdk",
        any(target_os = "windows", target_os = "linux")
    ))]
    fn sdk_config(&self) -> SdkSinkConfig {
        SdkSinkConfig {
            device_type: self.config.device_type.clone(),
            center_frequency: self.config.center_frequency,
            span_frequency: self.config.span_frequency,
            trans_gain: self.config.trans_gain,
            ..SdkSinkConfig::default()
        }
    }
}

impl Default for UnifiedSink {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for [`UnifiedSink`]. Platform-uniform: setters always exist;
/// whether the built sink can transmit is decided at
/// [`UnifiedSink::initialize`] time.
#[derive(Debug, Clone, Default)]
pub struct AaroniaSinkBuilder {
    config: UnifiedSinkConfig,
}

impl AaroniaSinkBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the TX center frequency in Hz.
    #[must_use]
    pub fn center_frequency(mut self, hz: f64) -> Self {
        self.config.center_frequency = hz;
        self
    }

    /// Set the IQ sample rate (span) in Hz.
    #[must_use]
    pub fn sample_rate(mut self, hz: f64) -> Self {
        self.config.span_frequency = hz;
        self
    }

    /// Set the transmission gain in dB.
    #[must_use]
    pub fn trans_gain(mut self, db: f64) -> Self {
        self.config.trans_gain = db;
        self
    }

    /// Set the SDK device family / open mode.
    #[must_use]
    pub fn device_type(mut self, device_type: String) -> Self {
        self.config.device_type = device_type;
        self
    }

    pub fn build(self) -> UnifiedSink {
        UnifiedSink::with_config(self.config)
    }
}
