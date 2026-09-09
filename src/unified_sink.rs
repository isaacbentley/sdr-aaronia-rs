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
    pub center_frequency_hz: f64,
    /// IQ sample rate (Fs) in Hz.
    pub sample_rate_hz: f64,
    /// Transmission gain in dB (typically 0.0 to -120.0).
    pub trans_gain_db: f64,
}

impl Default for UnifiedSinkConfig {
    fn default() -> Self {
        Self {
            device_type: "spectranv6".to_string(),
            center_frequency_hz: 1.0e9,
            sample_rate_hz: 10.0e6,
            trans_gain_db: -20.0,
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

    /// Whether this build has a transmit path at all.
    ///
    /// `UnifiedSink` constructs on every platform so that FFI consumers
    /// hold one type everywhere, and only [`Self::initialize`] fails
    /// where TX is unavailable. That is too late for a caller that has
    /// to *advertise* a capability before anything is opened — the
    /// SoapySDR plugin publishes its channel count at device
    /// construction — so the same compile-time condition is readable
    /// here, before a sink exists.
    ///
    /// Compile-time only: `true` means this binary carries the TX code,
    /// not that the attached device has a transmitter. A V6 ECO does
    /// not, and nothing here can tell.
    pub const fn tx_supported() -> bool {
        cfg!(all(
            feature = "native-sdk",
            any(target_os = "windows", target_os = "linux")
        ))
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
            // Re-initialising must not orphan a live backend: stop it so
            // its device is released before the replacement opens one.
            if let Some(mut old) = self.backend.take()
                && let Err(e) = old.stop_streaming().await
            {
                tracing::warn!("stopping the previous sink backend failed: {e}");
            }
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

    /// The device's master stream clock, in nanoseconds since the Unix
    /// epoch. TX burst times are expressed against this clock, not
    /// wall-clock time.
    ///
    /// Nanoseconds to match every other clock the crate reports;
    /// [`Self::send_burst`] takes the vendor's seconds, so convert with
    /// [`crate::utils::epoch_nanos_to_seconds`].
    pub fn master_stream_time_ns(&mut self) -> Result<i64> {
        #[cfg(all(
            feature = "native-sdk",
            any(target_os = "windows", target_os = "linux")
        ))]
        {
            let backend = self
                .backend
                .as_mut()
                .ok_or_else(|| Error::Sdk("Sink not initialized".to_string()))?;
            backend.master_stream_time_ns()
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
    /// seconds — the vendor's own unit for the packet header. Read the
    /// clock with [`Self::master_stream_time_ns`] and convert with
    /// [`crate::utils::epoch_nanos_to_seconds`]. `flags` are
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
    /// Aaronia's `IQTransceiverSweep` sample precedes real data with a
    /// zero-length packet carrying only `STREAM_START`, timestamped at
    /// the master stream clock, "to improve startup synch". Passing an
    /// empty slice here does the same thing. Untested against hardware,
    /// like the rest of this path, but it is what the vendor does.
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
            let config_center_frequency_hz = self.config.center_frequency_hz;
            let config_sample_rate_hz = self.config.sample_rate_hz;
            let backend = self
                .backend
                .as_mut()
                .ok_or_else(|| Error::Sdk("Sink not initialized".to_string()))?;
            let burst = crate::native_sdk::TxBurst {
                start_time: start_time_s,
                end_time: end_time_s,
                center_frequency_hz: config_center_frequency_hz,
                sample_rate_hz: config_sample_rate_hz,
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
            center_frequency_hz: self.config.center_frequency_hz,
            sample_rate_hz: self.config.sample_rate_hz,
            trans_gain_db: self.config.trans_gain_db,
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
pub struct SpectranSinkBuilder {
    config: UnifiedSinkConfig,
}

impl SpectranSinkBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the TX center frequency in Hz.
    #[must_use]
    pub fn center_frequency_hz(mut self, hz: f64) -> Self {
        self.config.center_frequency_hz = hz;
        self
    }

    /// Set the IQ sample rate (Fs) in Hz.
    #[must_use]
    pub fn sample_rate_hz(mut self, hz: f64) -> Self {
        self.config.sample_rate_hz = hz;
        self
    }

    /// Set the transmission gain in dB.
    #[must_use]
    pub fn trans_gain_db(mut self, db: f64) -> Self {
        self.config.trans_gain_db = db;
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
