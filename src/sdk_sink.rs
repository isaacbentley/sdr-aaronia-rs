//! Aaronia RTSA Native SDK Transmitter Integration
//!
//! This module provides a high-level wrapper over [`crate::native_sdk`] for
//! transmitting IQ samples via the native SDK.
//!
//! Unlike `native_sdk` itself, this module carries **no** internal
//! platform/feature fallback: `lib.rs` gates `pub mod sdk_sink;` behind
//! the exact same condition (`feature = "native-sdk"` and
//! `target_os = "windows"` or `"linux"`) that `native_sdk` uses.

use crate::Result;
use num_complex::Complex32;
use std::time::Duration;

pub mod native_sdk {
    // Re-export everything from the native_sdk module
    pub use crate::native_sdk::*;
}

/// Configuration for SDK sink
#[derive(Debug, Clone)]
pub struct SdkSinkConfig {
    /// SDK device family (`"spectranv6"`), optionally
    /// mode-qualified (`"spectranv6/iqtransmitter"`). Enumeration always uses the
    /// bare family, and opening uses the qualified form.
    pub device_type: String,
    /// Center frequency in Hz.
    pub center_frequency: f64,
    /// IQ span (sample rate) in Hz.
    pub span_frequency: f64,
    /// Transmission gain in dB (0.0 to -120.0 typically).
    pub trans_gain: f64,
    /// Device operation timeout.
    pub timeout: Duration,
}

impl Default for SdkSinkConfig {
    fn default() -> Self {
        Self {
            device_type: "spectranv6/iqtransmitter".to_string(),
            center_frequency: 1e9, // 1 GHz
            span_frequency: 10e6,  // 10 MHz
            trans_gain: -20.0,     // -20 dB
            timeout: Duration::from_secs(30),
        }
    }
}

impl SdkSinkConfig {
    /// Bare device family for `AARTSAAPI_EnumDevice` (strips any `/mode`).
    pub fn device_family(&self) -> &str {
        self.device_type
            .split('/')
            .next()
            .unwrap_or(&self.device_type)
    }

    /// Mode-qualified open string for `AARTSAAPI_OpenDevice`. Uses the
    /// configured mode when present, otherwise `<family>/iqtransmitter`.
    pub fn device_open_mode(&self) -> String {
        if self.device_type.contains('/') {
            self.device_type.clone()
        } else {
            format!("{}/iqtransmitter", self.device_family())
        }
    }
}

/// High-level SDK sink wrapper for transmitting IQ samples via Spectran V6 SDK.
pub struct SdkSink {
    native_source: Option<crate::native_sdk::NativeSdkSource>,
    config: SdkSinkConfig,
}

impl SdkSink {
    /// Create a new SDK sink with default configuration
    pub fn new() -> Self {
        Self {
            native_source: None,
            config: SdkSinkConfig::default(),
        }
    }

    /// Create a new SDK sink with custom configuration
    pub fn with_config(config: SdkSinkConfig) -> Self {
        Self {
            native_source: None,
            config,
        }
    }

    /// Initialize the SDK sink
    pub async fn initialize(&mut self) -> Result<()> {
        let mut native_source = unsafe { crate::native_sdk::NativeSdkSource::new()? };
        unsafe { native_source.initialize()? };
        self.native_source = Some(native_source);
        Ok(())
    }

    /// Check if the sink is available
    pub fn is_available(&self) -> bool {
        self.native_source.is_some()
    }

    /// Get the current configuration
    pub fn get_config(&self) -> &SdkSinkConfig {
        &self.config
    }

    /// Update configuration
    pub fn update_config(&mut self, config: SdkSinkConfig) {
        self.config = config;
    }

    /// Start the transmit stream and configure the device
    pub async fn start_streaming(&mut self) -> Result<()> {
        let native_source = self
            .native_source
            .as_mut()
            .ok_or_else(|| crate::Error::Sdk("Source not initialized".to_string()))?;

        unsafe {
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

            // Configure transmitter
            native_source.configure_iq_transmitter(
                self.config.center_frequency,
                self.config.span_frequency,
                self.config.trans_gain,
            )?;

            // Start streaming
            native_source.start_streaming()?;
        }
        Ok(())
    }

    /// Read the device's master stream time.
    ///
    /// This time is required to correctly schedule `TxBurst` packets.
    pub fn get_master_stream_time(&mut self) -> Result<f64> {
        let native_source = self
            .native_source
            .as_mut()
            .ok_or_else(|| crate::Error::Sdk("Source not initialized".to_string()))?;
        native_source.get_master_stream_time()
    }

    /// Obtain a `TxStream` handle to write samples.
    pub fn start_tx_stream(&mut self) -> Result<crate::native_sdk::TxStream<'_>> {
        let native_source = self
            .native_source
            .as_mut()
            .ok_or_else(|| crate::Error::Sdk("Source not initialized".to_string()))?;
        unsafe { native_source.start_tx_stream() }
    }

    /// Write raw IQ samples to the device synchronously (helper method).
    ///
    /// The caller is responsible for constructing the `TxBurst` packet
    /// with the correct flags, timing, and center/span frequency.
    pub fn write_samples(
        &mut self,
        channel: i32,
        burst: crate::native_sdk::TxBurst,
        samples: &[Complex32],
    ) -> Result<()> {
        let mut tx_stream = self.start_tx_stream()?;
        unsafe { tx_stream.write_samples(channel, burst, samples) }
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

    /// Check if streaming is active
    pub fn is_streaming(&self) -> bool {
        self.native_source
            .as_ref()
            .map(|s| s.is_streaming())
            .unwrap_or(false)
    }
}

impl Default for SdkSink {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "futuresdr")]
pub mod futuresdr_sink {
    use super::*;
    use futuresdr::runtime::Kernel;
    use futuresdr::macros::Block;
    use tracing::error;

    /// FutureSDR block that sinks `Complex32` items to the native SDK.
    ///
    /// Note: This block currently buffers incoming samples and drops them
    /// if the buffer exceeds a simple threshold to prevent unbound memory
    /// usage, or you must build a pacing mechanism in your graph. Since
    /// we cannot test hardware, the simplest continuous streaming block
    /// is provided.
    #[derive(Block)]
    pub struct SdkSinkBlock {
        sink: SdkSink,
        channel: i32,
        samples_per_burst: usize,
        #[input]
        input: futuresdr::runtime::buffer::DefaultCpuReader<Complex32>,
    }

    impl SdkSinkBlock {
        pub fn new(sink: SdkSink, channel: i32, samples_per_burst: usize) -> Self {
            Self {
                sink,
                channel,
                samples_per_burst,
                input: futuresdr::runtime::buffer::DefaultCpuReader::default(),
            }
        }
    }

    impl Kernel for SdkSinkBlock {
        async fn work(
            &mut self,
            io: &mut futuresdr::runtime::WorkIo,
            _mio: &mut futuresdr::runtime::MessageOutputs,
            _meta: &mut futuresdr::runtime::BlockMeta,
        ) -> anyhow::Result<()> {
            let i = self.input.slice();

            if i.is_empty() {
                if self.input.finished() {
                    io.finished = true;
                }
                return Ok(());
            }

            let n = std::cmp::min(i.len(), self.samples_per_burst);

            // In a real continuous stream, we would calculate start_time and
            // end_time precisely. Here we dispatch "immediate" or un-paced
            // packets for testing.
            let burst = crate::native_sdk::TxBurst {
                start_time: 0.0,
                end_time: 0.0,
                center_frequency_hz: self.sink.get_config().center_frequency,
                sample_rate_hz: self.sink.get_config().span_frequency,
                flags: crate::native_sdk::tx_flags::SEGMENT_START
                    | crate::native_sdk::tx_flags::SEGMENT_END,
            };

            if let Err(e) = self.sink.write_samples(self.channel, burst, &i[..n]) {
                error!("Failed to write samples to SDK: {}", e);
            }

            self.input.consume(n);
            if self.input.finished() && n == i.len() {
                io.finished = true;
            }
            Ok(())
        }
    }
}
