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
    pub center_frequency_hz: f64,
    /// IQ span (sample rate) in Hz.
    pub sample_rate_hz: f64,
    /// Transmission gain in dB (0.0 to -120.0 typically).
    pub trans_gain_db: f64,
    /// Device operation timeout.
    ///
    /// **Currently not applied.** Nothing in this wrapper or in
    /// [`crate::native_sdk`] reads this value; the transmit path
    /// ([`SdkSink::write_samples`] → `AARTSAAPI_SendPacket`) is a
    /// synchronous vendor call with no timeout parameter to pass it to.
    /// Setting it changes no behaviour.
    ///
    /// Kept rather than removed because the field is `pub` on a published
    /// crate, so dropping it would break struct-literal construction
    /// downstream for a knob nobody can be relying on. Mirrors
    /// [`crate::sdk_source::SdkConfig::timeout`].
    pub timeout: Duration,
}

impl Default for SdkSinkConfig {
    fn default() -> Self {
        Self {
            device_type: "spectranv6/iqtransmitter".to_string(),
            center_frequency_hz: 1e9, // 1 GHz
            sample_rate_hz: 10e6,     // 10 MHz
            trans_gain_db: -20.0,     // -20 dB
            timeout: Duration::from_secs(30),
        }
    }
}

impl SdkSinkConfig {
    /// Bare device family for `AARTSAAPI_EnumDevice` (strips any `/mode`).
    pub fn device_family(&self) -> &str {
        crate::native_sdk::split_device_type(&self.device_type, "iqtransmitter").0
    }

    /// Mode-qualified open string for `AARTSAAPI_OpenDevice`. Uses the
    /// configured mode when present, otherwise `<family>/iqtransmitter`.
    pub fn device_open_mode(&self) -> String {
        crate::native_sdk::split_device_type(&self.device_type, "iqtransmitter").1
    }
}

/// High-level SDK sink wrapper for transmitting IQ samples via Spectran V6 SDK.
pub struct SdkSink {
    native_source: Option<crate::native_sdk::NativeSdkSource>,
    config: SdkSinkConfig,
    device_serial: Option<String>,
}

impl SdkSink {
    /// Create a new SDK sink with default configuration
    pub fn new() -> Self {
        Self {
            native_source: None,
            config: SdkSinkConfig::default(),
            device_serial: None,
        }
    }

    /// Create a new SDK sink with custom configuration
    pub fn with_config(config: SdkSinkConfig) -> Self {
        Self {
            native_source: None,
            config,
            device_serial: None,
        }
    }

    /// Pin this sink to a specific device serial number.
    #[must_use]
    pub fn with_device_serial(mut self, serial: impl Into<String>) -> Self {
        self.device_serial = Some(serial.into());
        self
    }

    /// Set or clear the target device serial number.
    pub fn set_device_serial(&mut self, serial: Option<String>) {
        self.device_serial = serial;
    }

    /// Return the configured target device serial number, if any.
    pub fn device_serial(&self) -> Option<&str> {
        self.device_serial.as_deref()
    }

    /// Initialize the SDK sink
    pub async fn initialize(&mut self) -> Result<()> {
        let native_source = crate::utils::maybe_block_in_place(|| unsafe {
            let mut native_source = crate::native_sdk::NativeSdkSource::new()?;
            native_source.initialize()?;
            Ok::<_, crate::Error>(native_source)
        })?;
        self.native_source = Some(native_source);
        Ok(())
    }

    /// Check if the sink is available
    pub fn is_available(&self) -> bool {
        self.native_source.is_some()
    }

    /// Get the current configuration
    pub fn config(&self) -> &SdkSinkConfig {
        &self.config
    }

    /// Update configuration
    pub fn update_config(&mut self, config: SdkSinkConfig) {
        self.config = config;
    }

    /// Retune the TX center frequency in Hz (applied immediately if open).
    pub fn set_center_frequency_hz(&mut self, hz: f64) -> Result<()> {
        if !hz.is_finite() || hz <= 0.0 {
            return Err(crate::Error::Config(
                "TX center frequency must be finite and positive".into(),
            ));
        }
        if let Some(ref mut native_source) = self.native_source
            && native_source.is_device_open()
        {
            let sample_rate_hz = self.config.sample_rate_hz;
            let trans_gain_db = self.config.trans_gain_db;
            crate::utils::maybe_block_in_place(|| unsafe {
                native_source.configure_iq_transmitter(hz, sample_rate_hz, trans_gain_db)
            })?;
        }
        self.config.center_frequency_hz = hz;
        Ok(())
    }

    /// Update the TX IQ sample rate in Hz (applied immediately if open).
    pub fn set_sample_rate_hz(&mut self, hz: f64) -> Result<()> {
        if !hz.is_finite() || hz <= 0.0 {
            return Err(crate::Error::Config(
                "TX sample rate must be finite and positive".into(),
            ));
        }
        if let Some(ref mut native_source) = self.native_source
            && native_source.is_device_open()
        {
            let center_frequency_hz = self.config.center_frequency_hz;
            let trans_gain_db = self.config.trans_gain_db;
            crate::utils::maybe_block_in_place(|| unsafe {
                native_source.configure_iq_transmitter(center_frequency_hz, hz, trans_gain_db)
            })?;
        }
        self.config.sample_rate_hz = hz;
        Ok(())
    }

    /// Update the TX gain in dB (applied immediately if open).
    pub fn set_trans_gain_db(&mut self, db: f64) -> Result<()> {
        if !db.is_finite() {
            return Err(crate::Error::Config("TX gain must be finite".into()));
        }
        if let Some(ref mut native_source) = self.native_source
            && native_source.is_device_open()
        {
            let center_frequency_hz = self.config.center_frequency_hz;
            let sample_rate_hz = self.config.sample_rate_hz;
            crate::utils::maybe_block_in_place(|| unsafe {
                native_source.configure_iq_transmitter(center_frequency_hz, sample_rate_hz, db)
            })?;
        }
        self.config.trans_gain_db = db;
        Ok(())
    }

    /// Start the transmit stream and configure the device
    pub async fn start_streaming(&mut self) -> Result<()> {
        let native_source = self
            .native_source
            .as_mut()
            .ok_or_else(|| crate::Error::Sdk("Source not initialized".to_string()))?;
        let config = &self.config;
        let target_serial = self.device_serial.as_deref();

        crate::utils::maybe_block_in_place(|| unsafe {
            let family = config.device_family().to_string();
            let open_mode = config.device_open_mode();
            let devices = native_source.find_devices(&family)?;
            if devices.is_empty() {
                return Err(crate::Error::DeviceNotFound {
                    serial: target_serial.map(str::to_owned),
                    families: vec![family],
                });
            }

            let device_info = if let Some(serial) = target_serial {
                devices
                    .iter()
                    .find(|d| crate::native_sdk::NativeSdkSource::get_device_serial(d) == serial)
                    .ok_or_else(|| crate::Error::DeviceNotFound {
                        serial: Some(serial.to_owned()),
                        families: vec![family],
                    })?
            } else {
                devices
                    .iter()
                    .find(|d| d.ready() && !d.active())
                    .unwrap_or(&devices[0])
            };

            let selected_serial =
                crate::native_sdk::NativeSdkSource::get_device_serial(device_info);
            if !device_info.ready() {
                return Err(crate::Error::DeviceNotReady {
                    serial: selected_serial,
                });
            }
            if device_info.active() {
                return Err(crate::Error::SdkApi {
                    operation: "select transmit device".into(),
                    code: crate::native_sdk::SdkError::Busy,
                });
            }

            native_source.open_device(&open_mode, &device_info.serial_number)?;

            let setup_res = native_source
                .configure_iq_transmitter(
                    config.center_frequency_hz,
                    config.sample_rate_hz,
                    config.trans_gain_db,
                )
                .and_then(|()| native_source.start_streaming());
            if let Err(e) = setup_res {
                let _ = native_source.stop_streaming();
                return Err(e);
            }
            Ok(())
        })
    }

    /// The device's master stream clock, in nanoseconds since the Unix
    /// epoch. `TxBurst` times are expressed against this clock, not
    /// wall-clock time.
    pub fn master_stream_time_ns(&mut self) -> Result<i64> {
        let native_source = self
            .native_source
            .as_mut()
            .ok_or_else(|| crate::Error::Sdk("Source not initialized".to_string()))?;
        native_source.master_stream_time_ns()
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
        crate::utils::maybe_block_in_place(|| unsafe { native_source.stop_streaming() })
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
    // `BufferReader`/`CpuBufferReader` provide the `init`/`validate`/
    // `finish`/`notify_finished`/`slice`/`finished`/`consume` methods that
    // the `#[derive(Block)]` expansion below calls on the `#[input]`
    // field. The traits are implemented for `circular::Reader<D>` (the
    // concrete type behind `DefaultCpuReader`) but must be explicitly in
    // scope for the derive-generated code to see them.
    use futuresdr::macros::Block;
    use futuresdr::prelude::{BufferReader, CpuBufferReader};
    use futuresdr::runtime::Kernel;
    use tracing::{error, warn};

    /// How far ahead of the device's master stream time a burst is
    /// scheduled. The packet has to reach the device before its scheduled
    /// start or the scheduler has already passed it, so "now" exactly is
    /// always too late; this is the slack that covers the SendPacket call
    /// and the USB hop.
    ///
    /// Hardware-unverified — 10 ms is a conservative guess, not a measured
    /// figure. Too small and bursts arrive late; too large and they sit in
    /// the device queue adding latency.
    const TX_SCHEDULE_LEAD_S: f64 = 0.010;

    /// FutureSDR block that sinks `Complex32` items to the native SDK.
    ///
    /// Each `work()` call forwards up to `samples_per_burst` items as one
    /// `TxBurst` and consumes them. The block holds no queue of its own —
    /// back-pressure is whatever the upstream `DefaultCpuReader` provides.
    ///
    /// > [!WARNING]
    /// > Hardware-unverified, like the rest of the TX path — the
    /// > developer's V6 ECO has no transmitter. In particular the burst
    /// > scheduling below (lead time, one burst per `work()`) has never
    /// > been checked against a real device.
    ///
    /// **Samples are dropped if `AARTSAAPI_SendPacket` fails.** The error
    /// is logged and the input is consumed anyway: leaving it unconsumed
    /// would re-present the same failing samples on the next `work()` call
    /// and livelock the flowgraph on a persistently failing device. Callers
    /// needing guaranteed delivery should drive [`SdkSink::write_samples`]
    /// directly and handle the error.
    #[derive(Block)]
    pub struct SdkSinkBlock {
        sink: SdkSink,
        channel: i32,
        samples_per_burst: usize,
        next_tx_time_s: Option<f64>,
        #[input]
        input: futuresdr::runtime::buffer::DefaultCpuReader<Complex32>,
    }

    // SAFETY: `SdkSink` owns a `NativeSdkSource`, which in turn owns
    // `AARTSAAPI_Handle`/`AARTSAAPI_Device`/`AARTSAAPI_Config` — thin
    // wrappers around a raw `*mut c_void` into the vendor SDK's internal
    // state — so `Send`/`Sync` are not auto-derived. `futuresdr::runtime::
    // Kernel: Send` requires the whole block to be `Send` because the
    // executor may move a block to a different worker thread between
    // polls; `Kernel::work` takes `&mut self`, so the block is never
    // accessed concurrently from two threads at once (the same reasoning
    // `TxStream` above uses for its own `unsafe impl Send`). `Sync` is
    // deliberately not implemented, since nothing here takes `&self`
    // concurrently.
    //
    // What this does NOT verify: whether the Aaronia vendor SDK's C API
    // tolerates a device handle being *used* from a different OS thread
    // than the one that opened/connected it (some USB-backed SDKs pin
    // handle state to their opening thread). That is undocumented and
    // hardware-unverified — consistent with this crate's existing
    // caveats on other unverified hardware paths (`RxChannel::Rx2`/
    // `Rx1And2`, the TX burst path, `configure_sweepsa`). Revisit if a
    // real device exhibits cross-thread handle issues.
    unsafe impl Send for SdkSinkBlock {}

    impl SdkSinkBlock {
        pub fn new(sink: SdkSink, channel: i32, samples_per_burst: usize) -> Self {
            Self {
                sink,
                channel,
                samples_per_burst: samples_per_burst.max(1),
                next_tx_time_s: None,
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
            // `input_len` is captured as a plain `usize` (not a held slice
            // reference) so nothing borrows `self.input` across the later
            // `self.input.consume(n)` call below — the same shape
            // `HttpSink::work` uses for this exact `slice()`/`consume()`
            // pairing. Holding the slice past `consume()` would double-
            // borrow `self.input` (E0499/E0502).
            let input_len = self.input.slice().len();

            if input_len == 0 {
                if self.input.finished() {
                    io.finished = true;
                }
                return Ok(());
            }

            let n = std::cmp::min(input_len, self.samples_per_burst);

            // `TxBurst` documents that the device schedules a burst against
            // its own master stream clock and that zero timestamps are
            // wrong — `start_time: 0.0` asks the device to transmit at the
            // Unix epoch, decades in the past. Derive a monotonic window
            // from the device clock instead so consecutive bursts within
            // `TX_SCHEDULE_LEAD_S` are queued back-to-back rather than
            // overlapping in time.
            let sample_rate_hz = self.sink.config().sample_rate_hz;
            let duration_s = if sample_rate_hz > 0.0 {
                n as f64 / sample_rate_hz
            } else {
                0.0
            };
            let mut flags = crate::native_sdk::tx_flags::SEGMENT_START
                | crate::native_sdk::tx_flags::SEGMENT_END;
            let (start_time, end_time) = match self.sink.master_stream_time_ns() {
                Ok(now_ns) => {
                    // Back to the vendor's seconds here, at the one
                    // boundary that wants them: TxBurst feeds the packet
                    // header directly.
                    let min_start =
                        crate::utils::epoch_nanos_to_seconds(now_ns) + TX_SCHEDULE_LEAD_S;
                    let start = self.next_tx_time_s.map_or(min_start, |t| t.max(min_start));
                    let end = start + duration_s;
                    self.next_tx_time_s = Some(end);
                    (start, end)
                }
                Err(e) => {
                    // No usable clock: fall back to the SDK's documented
                    // "process this packet now" escape hatch rather than
                    // handing the scheduler an epoch-zero window.
                    warn!("Master stream time unavailable ({e}); dispatching burst with PUSH");
                    self.next_tx_time_s = None;
                    flags |= crate::native_sdk::tx_flags::PUSH;
                    (0.0, 0.0)
                }
            };

            let burst = crate::native_sdk::TxBurst {
                start_time,
                end_time,
                center_frequency_hz: self.sink.config().center_frequency_hz,
                sample_rate_hz,
                flags,
            };

            // Scoped: the slice borrow of `self.input` must end before
            // `self.input.consume(n)` runs.
            let write_result = {
                let i = self.input.slice();
                let samples = &i[..n];
                let channel = self.channel;
                let sink = &mut self.sink;
                crate::utils::maybe_block_in_place(|| sink.write_samples(channel, burst, samples))
            };
            if let Err(e) = write_result {
                error!("Failed to write samples to SDK: {}", e);
            }

            self.input.consume(n);
            if self.input.finished() && n == input_len {
                io.finished = true;
            } else if n < input_len {
                io.call_again = true;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sink's half of the family/mode split. It shares
    /// `split_device_type` with `SdkConfig` but supplies a different
    /// default mode, so the trailing-slash fix has to hold here too — and
    /// `iqtransmitter` is exactly the mode a malformed `"spectranv6/"`
    /// used to drop on the floor.
    #[test]
    fn a_trailing_slash_still_gets_the_transmit_mode() {
        let mut config = SdkSinkConfig {
            device_type: "spectranv6".to_string(),
            ..Default::default()
        };
        assert_eq!(config.device_family(), "spectranv6");
        assert_eq!(config.device_open_mode(), "spectranv6/iqtransmitter");

        config.device_type = "spectranv6/".to_string();
        assert_eq!(config.device_family(), "spectranv6");
        assert_eq!(config.device_open_mode(), "spectranv6/iqtransmitter");

        // An explicit mode is still passed through untouched. The sink has
        // no eco remap of its own — `iqtransmitter` is the only default.
        config.device_type = "spectranv6/iqtransceiver".to_string();
        assert_eq!(config.device_open_mode(), "spectranv6/iqtransceiver");
    }
}
