//! Unified Aaronia Source API
//!
//! This module provides a unified interface for accessing Aaronia devices
//! and data sources. It automatically detects and prioritizes available
//! connection methods in the following order:
//! 1. Native SDK (if installed and feature enabled)
//! 2. HTTP streaming (if device accessible via network)
//! 3. File sources (RTSA files)
//!
//! The goal is to provide a seamless developer experience where the
//! underlying connection method is abstracted away.

use crate::utils::{DEFAULT_RECEIVER_CLOCK_HZ, validate_iq_mode};
use crate::{Error, Result};
use futures::stream::StreamExt;
use num_complex::Complex32;
use std::collections::VecDeque;
use std::path::Path;
use std::time::Duration;
use tracing::{error, info, warn};

use crate::file_source::RtsaSource;
use crate::http_endpoints::HttpEndpointsClient;

#[cfg(all(
    feature = "native-sdk",
    any(target_os = "windows", target_os = "linux")
))]
use crate::native_sdk::NativeSdkSource;

#[cfg(all(
    feature = "native-sdk",
    any(target_os = "windows", target_os = "linux")
))]
use crate::detection::is_sdk_installed;

/// Represents the different types of Aaronia data sources
#[derive(Debug, Clone, PartialEq)]
pub enum SourceType {
    /// Native SDK connection (highest priority)
    NativeSdk,
    /// HTTP streaming connection
    Http,
    /// File-based source (RTSA files)
    File,
}

/// Configuration for the unified Aaronia source
#[derive(Debug, Clone)]
pub struct AaroniaConfig {
    /// Center frequency in Hz
    pub center_frequency: f64,
    /// IQ **sample rate** (Fs) in Hz. Named "span" for historical
    /// Aaronia-API reasons, but in IQ mode this is the sample rate, not
    /// the occupied RF bandwidth — the alias-free RX bandwidth is
    /// strictly smaller (see [`bandwidth_hz`](Self::bandwidth_hz)).
    pub span_frequency: f64,
    /// Usable **RX / real-time bandwidth** in Hz: the alias-free RF span
    /// actually captured. Strictly less than the sample rate
    /// `span_frequency` — the anti-alias filter rolls off the remaining
    /// fraction (e.g. 49.152 MHz usable inside a 61.44 MHz Fs capture).
    /// `0.0` means "unknown" (a live backend that hasn't reported it);
    /// file sources populate it from the RTSA sub-stream span.
    pub bandwidth_hz: f64,
    /// Reference level in dBm
    pub reference_level: f64,
    /// Device serial number (optional, uses first available if None)
    pub device_serial: Option<String>,
    /// HTTP base URL (for HTTP sources)
    pub http_base_url: Option<String>,
    /// File path (for file sources)
    pub file_path: Option<String>,
    /// Force a specific source type (skips auto-detection)
    pub force_source_type: Option<SourceType>,
    /// Stream format for HTTP sources
    pub stream_format: Option<crate::http_streaming::StreamFormat>,
    /// Stream scale for HTTP sources
    pub stream_scale: Option<f64>,
}

impl Default for AaroniaConfig {
    fn default() -> Self {
        Self {
            center_frequency: 2.44e9, // 2.44 GHz (ISM band)
            span_frequency: 15.36e6,  // 15.36 MHz sample rate
            bandwidth_hz: 0.0,        // unknown until a source reports it
            reference_level: -20.0,   // -20 dBm
            device_serial: None,
            http_base_url: None,
            file_path: None,
            force_source_type: None,
            stream_format: None,
            stream_scale: None,
        }
    }
}

impl AaroniaConfig {
    /// Create a new configuration for file source
    pub fn from_file<P: AsRef<Path>>(file_path: P) -> Self {
        Self {
            file_path: Some(file_path.as_ref().to_string_lossy().to_string()),
            force_source_type: Some(SourceType::File),
            ..Default::default()
        }
    }

    /// Create a new configuration for HTTP source
    pub fn from_http(base_url: &str) -> Self {
        Self {
            http_base_url: Some(base_url.to_string()),
            force_source_type: Some(SourceType::Http),
            ..Default::default()
        }
    }

    /// Force the use of native SDK
    #[must_use]
    pub fn force_native_sdk(mut self) -> Self {
        self.force_source_type = Some(SourceType::NativeSdk);
        self
    }

    /// Set the center frequency
    #[must_use]
    pub fn center_frequency(mut self, freq: f64) -> Self {
        self.center_frequency = freq;
        self
    }

    /// Set the span frequency
    #[must_use]
    pub fn span_frequency(mut self, freq: f64) -> Self {
        self.span_frequency = freq;
        self
    }

    /// Set the sample rate (alias for `span_frequency`)
    #[must_use]
    pub fn sample_rate_hz(self, freq: f64) -> Self {
        self.span_frequency(freq)
    }

    /// Set the reference level
    #[must_use]
    pub fn reference_level(mut self, level: f64) -> Self {
        self.reference_level = level;
        self
    }

    /// Set the HTTP wire format for `/stream` (default when unset:
    /// [`StreamFormat::Float32`](crate::http_streaming::StreamFormat)).
    /// Only meaningful for HTTP sources; ignored by file and native-SDK
    /// backends.
    #[must_use]
    pub fn stream_format(mut self, format: crate::http_streaming::StreamFormat) -> Self {
        self.stream_format = Some(format);
        self
    }

    /// Set the server-side `?scale=N` integer encode multiplier for
    /// `/stream`. Only meaningful for HTTP sources with an integer wire
    /// format.
    #[must_use]
    pub fn stream_scale(mut self, scale: f64) -> Self {
        self.stream_scale = Some(scale);
        self
    }

    /// Set the device serial number
    #[must_use]
    pub fn device_serial(mut self, serial: String) -> Self {
        self.device_serial = Some(serial);
        self
    }
}

/// Unified Aaronia source that automatically selects the best available connection method
pub struct AaroniaSource {
    config: AaroniaConfig,
    source_type: SourceType,
    sample_buffer: VecDeque<Complex32>,

    // Source-specific implementations
    #[cfg(all(
        feature = "native-sdk",
        any(target_os = "windows", target_os = "linux")
    ))]
    native_source: Option<NativeSdkSource>,

    http_client: Option<HttpEndpointsClient>,
    file_source: Option<RtsaSource>,
    /// Each item pairs a chunk of samples with whether the HTTP reader
    /// task's `DropDetector` observed a timestamp gap ending at (or
    /// before) that chunk — see `pending_overrun`.
    http_receiver: Option<tokio::sync::mpsc::Receiver<(Vec<Complex32>, bool)>>,
    /// Background task draining the HTTP `/stream` connection. Held so it
    /// can be aborted on `stop_streaming` / drop — otherwise it lingers
    /// parked on `next().await` (holding the open connection, so the
    /// device keeps streaming) until a packet happens to arrive and the
    /// dropped receiver is finally noticed.
    http_task: Option<tokio::task::JoinHandle<()>>,
    /// Latches `true` when a received HTTP chunk carried a detected
    /// drop, until [`Self::take_overrun`] reads and clears it. A flat
    /// per-call drop signal rather than a precise per-sample one: once
    /// chunks are merged into `sample_buffer`, the exact boundary a
    /// drop occurred at is no longer recoverable, so any drop observed
    /// since the last check is reported on the next `read_samples`
    /// call. Only the HTTP backend populates this today; the native
    /// SDK and file backends always report `false` (see
    /// [`Self::take_overrun`]).
    pending_overrun: bool,
}

impl AaroniaSource {
    /// Create a new unified Aaronia source with automatic detection
    pub async fn new(config: AaroniaConfig) -> Result<Self> {
        let mut source = Self {
            config: config.clone(),
            source_type: SourceType::Http, // Will be updated during detection
            sample_buffer: VecDeque::new(),

            #[cfg(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            ))]
            native_source: None,

            http_client: None,
            file_source: None,
            http_receiver: None,
            http_task: None,
            pending_overrun: false,
        };

        // Determine the best source type
        let source_type = if let Some(forced_type) = &config.force_source_type {
            info!("Forcing source type: {:?}", forced_type);
            forced_type.clone()
        } else {
            source.detect_best_source_type().await?
        };

        source.source_type = source_type.clone();

        // Hardware-bound source types must respect the IQ Mode
        // constraint. File sources read pre-recorded samples and aren't
        // subject to it.
        match source_type {
            SourceType::Http => {
                validate_iq_mode(config.span_frequency, DEFAULT_RECEIVER_CLOCK_HZ)?;
            }
            SourceType::NativeSdk | SourceType::File => {}
        }

        // Initialize the selected source
        match source_type {
            #[cfg(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            ))]
            SourceType::NativeSdk => {
                source.init_native_sdk().await?;
            }
            #[cfg(not(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            )))]
            SourceType::NativeSdk => {
                return Err(Error::Config(
                    "Native SDK not available on this platform or feature not enabled".to_string(),
                ));
            }
            SourceType::Http => {
                source.init_http_source().await?;
            }
            SourceType::File => {
                source.init_file_source().await?;
            }
        }

        info!("AaroniaSource initialized with {:?} backend", source_type);
        Ok(source)
    }

    /// Detect the best available source type based on configuration
    async fn detect_best_source_type(&self) -> Result<SourceType> {
        info!("Detecting Aaronia source type based on configuration...");

        // Rule 1: If file path provided, use file source only (no fallback)
        if let Some(file_path) = &self.config.file_path {
            if Path::new(file_path).exists() {
                info!("File path provided - using file source: {}", file_path);
                return Ok(SourceType::File);
            } else {
                return Err(Error::Config(format!(
                    "File path provided but file not found: {}",
                    file_path
                )));
            }
        }

        // Rule 2: If HTTP endpoint provided, use HTTP source only (no fallback)
        if let Some(base_url) = &self.config.http_base_url {
            info!("HTTP endpoint provided - using HTTP source: {}", base_url);
            return Ok(SourceType::Http);
        }

        // Rule 3: No specific source provided - auto-detect SDK or fallback to localhost HTTP
        info!("No specific source provided - checking SDK then localhost HTTP fallback");

        // Check for Native SDK first
        #[cfg(all(
            feature = "native-sdk",
            any(target_os = "windows", target_os = "linux")
        ))]
        {
            if is_sdk_installed() {
                info!("Native SDK detected and available");
                return Ok(SourceType::NativeSdk);
            } else {
                info!("Native SDK not installed - falling back to localhost HTTP");
            }
        }

        #[cfg(not(all(
            feature = "native-sdk",
            any(target_os = "windows", target_os = "linux")
        )))]
        {
            info!("Native SDK not supported on this platform - falling back to localhost HTTP");
        }

        // Fallback to localhost HTTP
        info!("Using localhost HTTP fallback: http://localhost:54664");
        Ok(SourceType::Http)
    }

    /// Initialize native SDK source
    #[cfg(all(
        feature = "native-sdk",
        any(target_os = "windows", target_os = "linux")
    ))]
    async fn init_native_sdk(&mut self) -> Result<()> {
        info!("Initializing Native SDK source");

        unsafe {
            let mut source = NativeSdkSource::new()?;
            source.initialize()?;

            // Per the official RTSA-API-Samples (EnumDevices.cpp:33,
            // RawIQ.cpp:101), `AARTSAAPI_EnumDevice` takes the bare device
            // family ("spectranv6"); only `AARTSAAPI_OpenDevice` takes the
            // mode-qualified form ("spectranv6/raw"). Passing the qualified
            // form to enumeration causes the SDK to silently return zero
            // devices.
            let device_family = "spectranv6";
            let open_mode = "spectranv6/raw";

            let devices = source.find_devices(device_family)?;

            if devices.is_empty() {
                return Err(Error::Config("No Spectran V6 devices found".to_string()));
            }

            // Select device (use specified serial or first available)
            let device_info = if let Some(ref serial) = self.config.device_serial {
                devices
                    .iter()
                    .find(|d| NativeSdkSource::get_device_serial(d) == *serial)
                    .ok_or_else(|| {
                        Error::Config(format!("Device with serial '{}' not found", serial))
                    })?
            } else {
                &devices[0]
            };

            // `WideChar` is u16 on Windows and u32 on Linux; widen
            // through u32 first so the cast compiles on both targets.
            let serial_wide: Vec<widestring::WideChar> =
                NativeSdkSource::get_device_serial(device_info)
                    .encode_utf16()
                    .chain([0])
                    .map(|c| c as widestring::WideChar)
                    .collect();

            // Open and configure device
            source.open_device(open_mode, &serial_wide)?;
            source.configure_iq_receiver(
                self.config.center_frequency,
                self.config.span_frequency,
                self.config.reference_level,
            )?;

            self.native_source = Some(source);
        }

        Ok(())
    }

    /// Initialize HTTP source
    async fn init_http_source(&mut self) -> Result<()> {
        info!("Initializing HTTP source");

        // Use provided URL or default to localhost:54664
        let base_url = self.config.http_base_url.clone().unwrap_or_else(|| {
            info!("No HTTP URL provided, using localhost default: http://localhost:54664");
            "http://localhost:54664".to_string()
        });

        let client =
            HttpEndpointsClient::new(base_url.clone(), crate::http_endpoints::AuthMethod::None)?;

        // Test connection immediately
        (client.test_connection().await).map_err(|e| Error::Protocol(format!("Failed to connect to Aaronia RTSA Suite Pro at {}. Is it running and accessible? Error: {}", base_url, e)))?;

        // Tune the hardware to the requested frequency *before* opening the
        // stream.  Without this the `/stream` endpoint returns whatever the
        // RTSA Suite is already configured to, completely ignoring the
        // caller's `center_frequency` / `span_frequency`.
        info!(
            "Tuning HTTP source to center={:.3} MHz, span={:.3} MHz, ref_level={} dBm",
            self.config.center_frequency / 1e6,
            self.config.span_frequency / 1e6,
            self.config.reference_level,
        );
        client
            .configure_capture(crate::http_endpoints::CaptureControl {
                frequency_center: Some(self.config.center_frequency),
                frequency_span: Some(self.config.span_frequency),
                reference_level: Some(self.config.reference_level as f32),
                control_type: crate::http_endpoints::ControlType::Capture,
                ..Default::default()
            })
            .await?;

        // Create a channel for sending samples from the async task to
        // AaroniaSource. Each item also carries whether the reader
        // task's DropDetector flagged a gap ending at this packet, so
        // `read_samples` can surface it as `IqPacket::overrun`.
        let (sender, receiver) = tokio::sync::mpsc::channel(100); // Buffer up to 100 chunks

        // Honour the caller's requested wire format; default to Float32
        // (binary, lossless) rather than JSON, which is the slowest format
        // and only appropriate for debugging.
        let stream_format = self
            .config
            .stream_format
            .unwrap_or(crate::http_streaming::StreamFormat::Float32);
        let mut params_builder = HttpEndpointsClient::stream_params().format(stream_format);
        if let Some(scale) = self.config.stream_scale {
            params_builder = params_builder.scale(scale);
        }
        let stream_params = params_builder.build();
        // Reuse the existing client for the reader task rather than building a
        // second one: `HttpEndpointsClient` clones cheaply (its inner
        // `reqwest::Client` is `Arc`-backed), so both share one connection
        // pool instead of standing up a duplicate.
        let client_for_task = client.clone();

        // Spawn a task to continuously read from the HTTP stream and send samples
        let reader_task = tokio::spawn(async move {
            let mut http_stream = match client_for_task.start_stream(stream_params).await {
                Ok(stream) => stream,
                Err(e) => {
                    error!("Failed to start HTTP stream: {:?}", e);
                    return;
                }
            };

            // Parse errors are recoverable (the parser resyncs at the next
            // packet header), so don't kill the stream on the first one —
            // only bail after a run of consecutive failures, which
            // indicates the transport itself is broken.
            const MAX_CONSECUTIVE_STREAM_ERRORS: u32 = 5;
            let mut consecutive_errors = 0u32;
            let mut drop_detector = crate::http_streaming::DropDetector::default();

            loop {
                match http_stream.next().await {
                    Some(Ok(packet)) => {
                        consecutive_errors = 0;
                        // The HTTP stream already returns parsed StreamPacket objects
                        if packet.metadata.payload == crate::http_streaming::PayloadType::Iq {
                            let dropped = matches!(
                                drop_detector.observe(&packet),
                                crate::http_streaming::DropResult::Drop { .. }
                            );
                            if sender.send((packet.samples, dropped)).await.is_err() {
                                // Receiver dropped, task can exit
                                info!("Receiver dropped, HTTP streaming task exiting.");
                                return;
                            }
                        } else {
                            warn!(
                                "HTTP streaming task received non-IQ payload type: {:?}",
                                packet.metadata.payload
                            );
                        }
                    }
                    Some(Err(e)) => {
                        consecutive_errors += 1;
                        if consecutive_errors >= MAX_CONSECUTIVE_STREAM_ERRORS {
                            error!(
                                "HTTP stream failed {} times in a row, giving up: {:?}",
                                consecutive_errors, e
                            );
                            break;
                        }
                        warn!("Recoverable HTTP stream error: {:?}", e);
                    }
                    None => {
                        info!("HTTP stream ended.");
                        break; // Stream ended
                    }
                }
            }
        });

        self.http_client = Some(client);
        self.http_receiver = Some(receiver); // Store the receiver
        self.http_task = Some(reader_task); // Held so stop/drop can abort it

        info!("HTTP client initialized for: {}", base_url);

        Ok(())
    }

    /// Initialize file source
    async fn init_file_source(&mut self) -> Result<()> {
        info!("Initializing file source");

        let file_path = self
            .config
            .file_path
            .as_ref()
            .ok_or_else(|| Error::Config("File path required for file source".to_string()))?;

        let source = RtsaSource::open(file_path)?;
        // RTSA files are authoritative for their own tuning: the
        // orchestrator passes placeholder center/span values for file
        // backends (it can't know them ahead of time), so pull the real
        // sample rate *and* center frequency out of the parsed chunk
        // metadata. Without the center-frequency propagation,
        // `get_source_info()` (and every emitted packet) reports 0 Hz,
        // so any absolute frequency a downstream consumer derives — e.g.
        // a DJI OcuSync detection — is meaningless.
        let meta = source.metadata();
        self.config.span_frequency = meta.sample_rate;
        // The RTSA sub-stream span is the usable RX bandwidth, distinct
        // from (and smaller than) the sample rate above. Surface it so
        // consumers don't mistake the sample rate for the captured RF
        // window.
        if meta.bandwidth > 0.0 {
            self.config.bandwidth_hz = meta.bandwidth;
        }
        if let Some(center) = meta.center_frequency
            && center > 0.0
        {
            self.config.center_frequency = center;
        }
        self.file_source = Some(source);

        Ok(())
    }

    /// Start streaming from the source
    pub async fn start_streaming(&mut self) -> Result<()> {
        info!("▶ Starting streaming with {:?} backend", self.source_type);

        match self.source_type {
            #[cfg(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            ))]
            SourceType::NativeSdk => {
                if let Some(ref mut source) = self.native_source {
                    unsafe { source.start_streaming()? };
                }
            }
            #[cfg(not(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            )))]
            SourceType::NativeSdk => {
                return Err(Error::Config("Native SDK not available".to_string()));
            }
            SourceType::Http => {
                // HTTP streaming is automatically started during initialization
                // Verify that the receiver channel is still active
                if self.http_receiver.is_some() {
                    info!("HTTP streaming confirmed active");
                } else {
                    return Err(Error::Config(
                        "HTTP streaming not properly initialized".to_string(),
                    ));
                }
            }
            SourceType::File => {
                // File sources are always "streaming" (reading from file)
                info!("File source ready for reading");
            }
        }

        Ok(())
    }

    /// Read IQ samples from the source
    pub async fn read_samples(
        &mut self,
        buffer: &mut Vec<Complex32>,
        max_samples: usize,
    ) -> Result<usize> {
        match self.source_type {
            #[cfg(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            ))]
            SourceType::NativeSdk => {
                if let Some(ref mut source) = self.native_source {
                    unsafe { source.read_samples(buffer, max_samples) }
                } else {
                    Err(Error::Config(
                        "Native SDK source not initialized".to_string(),
                    ))
                }
            }
            #[cfg(not(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            )))]
            SourceType::NativeSdk => Err(Error::Config("Native SDK not available".to_string())),
            SourceType::Http => {
                let start_len = buffer.len();
                let receiver = self
                    .http_receiver
                    .as_mut()
                    .ok_or_else(|| Error::Config("HTTP receiver not initialized".to_string()))?;

                // 1. Drain from internal buffer first
                let from_buffer = self.sample_buffer.len().min(max_samples);
                if from_buffer > 0 {
                    buffer.extend(self.sample_buffer.drain(0..from_buffer));
                }

                // 2. If we need more, receive from channel
                let mut collected = from_buffer;
                while collected < max_samples {
                    // If buffer is empty, try to receive more samples from the channel
                    match tokio::time::timeout(Duration::from_secs(30), receiver.recv()).await {
                        Ok(Some((mut new_samples, dropped))) => {
                            if dropped {
                                self.pending_overrun = true;
                            }
                            let needed = max_samples - collected;
                            if new_samples.len() <= needed {
                                collected += new_samples.len();
                                buffer.append(&mut new_samples);
                            } else {
                                collected += needed;
                                buffer.extend(new_samples.drain(0..needed));
                                // `sample_buffer` is a VecDeque, so it can't
                                // accept `Vec::append` — extend from the
                                // remainder instead.
                                self.sample_buffer.extend(new_samples);
                            }
                        }
                        Ok(None) => {
                            info!("HTTP sample channel closed.");
                            break; // Channel closed
                        }
                        Err(_) => {
                            tracing::warn!("Receive timeout");
                            break;
                        }
                    }
                }
                Ok(buffer.len() - start_len)
            }
            SourceType::File => {
                if let Some(ref mut source) = self.file_source {
                    // Read samples from file - fix the API call
                    let sample_data = source.read_samples(max_samples, None)?;
                    if let Some(data) = sample_data {
                        match data {
                            crate::file_source::SampleData::Iq(samples) => {
                                let n = samples.len();
                                buffer.extend(samples);
                                Ok(n)
                            }
                            _ => {
                                warn!("File contains non-IQ data, cannot convert to Complex32");
                                Ok(0)
                            }
                        }
                    } else {
                        Ok(0)
                    }
                } else {
                    Err(Error::Config("File source not initialized".to_string()))
                }
            }
        }
    }

    /// Read and clear whether a drop was detected since the last call.
    /// Currently only the HTTP backend populates this (via the reader
    /// task's `DropDetector`, watching for timestamp gaps between
    /// packets); file and native-SDK backends always return `false`.
    /// Callers (e.g. `SdrSource` implementations) call this once per
    /// `read_samples` to tag the resulting `IqPacket::overrun`.
    pub fn take_overrun(&mut self) -> bool {
        std::mem::take(&mut self.pending_overrun)
    }

    /// Stop streaming
    pub async fn stop_streaming(&mut self) -> Result<()> {
        info!("⏹ Stopping streaming");

        match self.source_type {
            #[cfg(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            ))]
            SourceType::NativeSdk => {
                if let Some(ref mut source) = self.native_source {
                    unsafe { source.stop_streaming()? };
                }
            }
            #[cfg(not(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            )))]
            SourceType::NativeSdk => {
                return Err(Error::Config("Native SDK not available".to_string()));
            }
            SourceType::Http => {
                // Abort the background reader so the open `/stream` connection
                // closes immediately — dropping the receiver alone only stops
                // the task the next time a packet arrives, so an idle stream
                // would keep the device streaming. Then drop the receiver.
                if let Some(task) = self.http_task.take() {
                    task.abort();
                }
                self.http_receiver = None;
                info!("HTTP streaming stopped: reader task aborted, receiver dropped.");
            }
            SourceType::File => {
                // No explicit stop needed for file sources
                info!("File source streaming stopped");
            }
        }

        Ok(())
    }

    /// Retune the source to a new centre frequency without rebuilding it.
    ///
    /// - **HTTP**: wraps `HttpEndpointsClient::configure_capture(frequency_center=freq)`.
    ///   Note this requires the RTSA-Suite "Remote Config" license; on
    ///   licenseless installs the PUT returns success but is silently ignored
    ///   server-side. Callers that need a license check should call
    ///   [`Self::probe_remote_config_license`] separately.
    /// - **Native SDK**: re-issues `configure_iq_receiver` with the new
    ///   centre frequency, the existing span, and the existing reference
    ///   level. The SDK config system applies the change to the open device
    ///   handle without a stream restart.
    /// - **File**: logs a warning and returns `Ok(())`. RTSA capture files
    ///   carry their own frequency in metadata; mid-stream retune is
    ///   meaningless. Hopping orchestrators shouldn't drive a file backend,
    ///   but the no-op keeps the abstraction tidy.
    pub async fn set_center_frequency(&mut self, freq: f64) -> Result<()> {
        match self.source_type {
            #[cfg(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            ))]
            SourceType::NativeSdk => {
                if let Some(ref mut source) = self.native_source {
                    unsafe {
                        source.configure_iq_receiver(
                            freq,
                            self.config.span_frequency,
                            self.config.reference_level,
                        )?
                    };
                } else {
                    return Err(Error::Config(
                        "Native SDK source not initialized".to_string(),
                    ));
                }
            }
            #[cfg(not(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            )))]
            SourceType::NativeSdk => {
                return Err(Error::Config("Native SDK not available".to_string()));
            }
            SourceType::Http => {
                let client = self
                    .http_client
                    .as_ref()
                    .ok_or_else(|| Error::Config("HTTP client not initialized".to_string()))?;
                client
                    .configure_capture(crate::http_endpoints::CaptureControl {
                        frequency_center: Some(freq),
                        control_type: crate::http_endpoints::ControlType::Capture,
                        ..Default::default()
                    })
                    .await?;
            }
            SourceType::File => {
                warn!(
                    "set_center_frequency called on file source (no-op); RTSA files carry their own frequency"
                );
            }
        }
        self.config.center_frequency = freq;
        Ok(())
    }

    /// Probe the RTSA-Suite Remote Config license status, returning
    /// [`crate::http_endpoints::RemoteConfigStatus::Active`] for non-HTTP
    /// sources (they don't need it). Hopping orchestrators should check
    /// this before relying on mid-stream `set_center_frequency` — without
    /// the license, `configure_capture` returns HTTP 200 OK server-side
    /// but silently ignores the frequency change, which would otherwise
    /// cause downstream packets to be mis-tagged with the wrong channel.
    ///
    /// **On HTTP sources this is an active probe that temporarily
    /// perturbs device state**: it adjusts the reference level by +1 dB
    /// and restores it best-effort (see
    /// [`HttpEndpointsClient::probe_remote_config_write_license`]). That
    /// is the only way to positively confirm write capability — an
    /// unlicensed write returns 200 OK and is silently ignored. Callers
    /// that must not touch the device should use
    /// [`HttpEndpointsClient::detect_remote_config_license`] directly and
    /// accept its weaker, read-only answer.
    pub async fn probe_remote_config_license(
        &self,
    ) -> Result<crate::http_endpoints::RemoteConfigStatus> {
        match self.source_type {
            SourceType::Http => {
                let client = self
                    .http_client
                    .as_ref()
                    .ok_or_else(|| Error::Config("HTTP client not initialized".to_string()))?;
                Ok(client.probe_remote_config_write_license().await)
            }
            _ => Ok(crate::http_endpoints::RemoteConfigStatus::Active),
        }
    }

    /// Deprecated alias for [`Self::probe_remote_config_license`].
    ///
    /// Renamed because the "is…" phrasing hid that, on HTTP sources, this
    /// actively perturbs device state (temporary +1 dB reference-level
    /// change) to verify write capability.
    #[deprecated(
        since = "0.1.0",
        note = "renamed to `probe_remote_config_license` — on HTTP sources this is an \
                active probe that temporarily changes the device reference level"
    )]
    pub async fn is_remote_config_licensed(
        &self,
    ) -> Result<crate::http_endpoints::RemoteConfigStatus> {
        self.probe_remote_config_license().await
    }

    /// Get information about the current source
    pub fn get_source_info(&self) -> SourceInfo {
        SourceInfo {
            source_type: self.source_type.clone(),
            center_frequency: self.config.center_frequency,
            span_frequency: self.config.span_frequency,
            bandwidth_hz: self.config.bandwidth_hz,
            reference_level: self.config.reference_level,
            device_serial: self.config.device_serial.clone(),
        }
    }

    /// Get the current configuration
    pub fn get_config(&self) -> &AaroniaConfig {
        &self.config
    }

    /// Check if the source is currently streaming
    pub fn is_streaming(&self) -> bool {
        match self.source_type {
            #[cfg(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            ))]
            SourceType::NativeSdk => self
                .native_source
                .as_ref()
                .map(|s| s.is_streaming())
                .unwrap_or(false),
            #[cfg(not(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            )))]
            SourceType::NativeSdk => false,
            SourceType::Http => self.http_receiver.is_some(),
            SourceType::File => {
                // File sources are always considered "streaming"
                self.file_source.is_some()
            }
        }
    }
}

impl Drop for AaroniaSource {
    fn drop(&mut self) {
        // Abort the background HTTP reader (if any) so a dropped source
        // doesn't leave a task parked on `next().await` holding the open
        // `/stream` connection. Aborting is enough — the task owns its
        // client and stream, both of which drop when it unwinds.
        if let Some(task) = self.http_task.take() {
            task.abort();
        }
    }
}

/// Information about the current Aaronia source
#[derive(Debug, Clone)]
pub struct SourceInfo {
    pub source_type: SourceType,
    pub center_frequency: f64,
    /// IQ sample rate (Fs) in Hz — see [`AaroniaConfig::span_frequency`].
    pub span_frequency: f64,
    /// Usable RX/real-time bandwidth in Hz (`0.0` = unknown) — see
    /// [`AaroniaConfig::bandwidth_hz`]. Always `<= span_frequency`.
    pub bandwidth_hz: f64,
    pub reference_level: f64,
    pub device_serial: Option<String>,
}

impl std::fmt::Display for SourceInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `span_frequency` is the sample rate (Fs); the RX bandwidth is a
        // separate, smaller number, only shown when the source reported it.
        write!(
            f,
            "Aaronia Source ({:?}): {:.1} MHz center, {:.1} MHz sample-rate",
            self.source_type,
            self.center_frequency / 1e6,
            self.span_frequency / 1e6,
        )?;
        if self.bandwidth_hz > 0.0 {
            write!(f, ", {:.3} MHz RX-BW", self.bandwidth_hz / 1e6)?;
        }
        write!(f, ", {:.1} dBm ref", self.reference_level)
    }
}

impl SourceInfo {
    /// Get the sample rate (alias for `span_frequency`)
    pub fn sample_rate_hz(&self) -> f64 {
        self.span_frequency
    }
}

/// Builder pattern for easy AaroniaSource configuration
pub struct AaroniaSourceBuilder {
    config: AaroniaConfig,
}

impl AaroniaSourceBuilder {
    /// Create a new builder with default configuration
    pub fn new() -> Self {
        Self {
            config: AaroniaConfig::default(),
        }
    }

    /// Set the center frequency
    pub fn center_frequency(&mut self, freq: f64) -> &mut Self {
        self.config.center_frequency = freq;
        self
    }

    /// Set the span frequency
    pub fn span_frequency(&mut self, freq: f64) -> &mut Self {
        self.config.span_frequency = freq;
        self
    }

    /// Set the sample rate (alias for `span_frequency`)
    pub fn sample_rate_hz(&mut self, freq: f64) -> &mut Self {
        self.span_frequency(freq)
    }

    /// Set the reference level
    pub fn reference_level(&mut self, level: f64) -> &mut Self {
        self.config.reference_level = level;
        self
    }

    /// Set the device serial number
    pub fn device_serial(&mut self, serial: String) -> &mut Self {
        self.config.device_serial = Some(serial);
        self
    }

    /// Configure for HTTP source
    pub fn http_source(&mut self, base_url: String) -> &mut Self {
        self.config.http_base_url = Some(base_url);
        self
    }

    /// Configure for file source
    pub fn file_source<P: AsRef<Path>>(&mut self, file_path: P) -> &mut Self {
        self.config.file_path = Some(file_path.as_ref().to_string_lossy().to_string());
        self
    }

    /// Force a specific source type
    pub fn force_source_type(&mut self, source_type: SourceType) -> &mut Self {
        self.config.force_source_type = Some(source_type);
        self
    }

    /// Set the HTTP `/stream` wire format (default when unset: Float32).
    /// Only meaningful for HTTP sources.
    pub fn stream_format(&mut self, format: crate::http_streaming::StreamFormat) -> &mut Self {
        self.config.stream_format = Some(format);
        self
    }

    /// Set the server-side `?scale=N` integer encode multiplier for the
    /// HTTP `/stream` endpoint. Only meaningful for HTTP sources with an
    /// integer wire format.
    pub fn stream_scale(&mut self, scale: f64) -> &mut Self {
        self.config.stream_scale = Some(scale);
        self
    }

    /// Build the AaroniaSource
    pub async fn build(&self) -> Result<AaroniaSource> {
        AaroniaSource::new(self.config.clone()).await
    }
}

impl Default for AaroniaSourceBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::NamedTempFile;

    #[test]
    fn test_source_type_enum() {
        // Test SourceType enum variants
        let native = SourceType::NativeSdk;
        let http = SourceType::Http;
        let file = SourceType::File;

        assert_eq!(native, SourceType::NativeSdk);
        assert_eq!(http, SourceType::Http);
        assert_eq!(file, SourceType::File);

        // Test Debug and Clone traits
        let cloned_native = native.clone();
        assert_eq!(native, cloned_native);
        println!("SourceType debug: {:?}", native);
    }

    #[test]
    fn test_aaronia_config_default() {
        // Test default configuration values
        let config = AaroniaConfig::default();

        assert_eq!(config.center_frequency, 2.44e9);
        assert_eq!(config.span_frequency, 15.36e6);
        assert_eq!(config.reference_level, -20.0);
        assert!(config.device_serial.is_none());
        assert!(config.http_base_url.is_none());
        assert!(config.file_path.is_none());
        assert!(config.force_source_type.is_none());
        assert!(config.stream_format.is_none());
        assert!(config.stream_scale.is_none());
    }

    #[test]
    fn test_aaronia_config_from_file() {
        // Test file configuration creation
        let file_path = "/path/to/test.rtsa";
        let config = AaroniaConfig::from_file(file_path);

        assert_eq!(config.file_path, Some(file_path.to_string()));
        assert_eq!(config.force_source_type, Some(SourceType::File));
        assert_eq!(config.center_frequency, 2.44e9); // Should inherit defaults
    }

    #[test]
    fn test_aaronia_config_from_http() {
        // Test HTTP configuration creation
        let base_url = "http://rtsa-device:54664";
        let config = AaroniaConfig::from_http(base_url);

        assert_eq!(config.http_base_url, Some(base_url.to_string()));
        assert_eq!(config.force_source_type, Some(SourceType::Http));
        assert_eq!(config.span_frequency, 15.36e6); // Should inherit defaults
    }

    #[test]
    fn test_aaronia_config_builder_methods() {
        // Test configuration builder pattern methods
        let config = AaroniaConfig::default()
            .center_frequency(915e6)
            .span_frequency(10e6)
            .reference_level(-30.0)
            .device_serial("TEST123".to_string())
            .force_native_sdk();

        assert_eq!(config.center_frequency, 915e6);
        assert_eq!(config.span_frequency, 10e6);
        assert_eq!(config.reference_level, -30.0);
        assert_eq!(config.device_serial, Some("TEST123".to_string()));
        assert_eq!(config.force_source_type, Some(SourceType::NativeSdk));
    }

    #[test]
    fn test_aaronia_source_builder_creation() {
        // Test builder creation and default values
        let builder = AaroniaSourceBuilder::new();
        assert_eq!(builder.config.center_frequency, 2.44e9);
        assert_eq!(builder.config.span_frequency, 15.36e6);

        let default_builder = AaroniaSourceBuilder::default();
        assert_eq!(default_builder.config.center_frequency, 2.44e9);
    }

    #[test]
    fn test_aaronia_source_builder_configuration() {
        // Test builder configuration methods
        let mut builder = AaroniaSourceBuilder::new();
        builder
            .center_frequency(2.4e9)
            .span_frequency(25e6)
            .reference_level(-25.0)
            .device_serial("DEV456".to_string())
            .http_source("http://localhost:8080".to_string())
            .force_source_type(SourceType::Http);

        assert_eq!(builder.config.center_frequency, 2.4e9);
        assert_eq!(builder.config.span_frequency, 25e6);
        assert_eq!(builder.config.reference_level, -25.0);
        assert_eq!(builder.config.device_serial, Some("DEV456".to_string()));
        assert_eq!(
            builder.config.http_base_url,
            Some("http://localhost:8080".to_string())
        );
        assert_eq!(builder.config.force_source_type, Some(SourceType::Http));
    }

    #[test]
    fn test_aaronia_source_builder_file_source() {
        // Test file source configuration
        let test_path = "/tmp/test_recording.rtsa";
        let mut builder = AaroniaSourceBuilder::new();
        builder.file_source(test_path);

        assert_eq!(builder.config.file_path, Some(test_path.to_string()));
    }

    #[test]
    fn test_source_info_creation() {
        // Test SourceInfo structure
        let info = SourceInfo {
            source_type: SourceType::Http,
            center_frequency: 2.45e9,
            span_frequency: 20e6,
            bandwidth_hz: 16e6,
            reference_level: -20.0,
            device_serial: Some("SERIAL123".to_string()),
        };

        assert_eq!(info.source_type, SourceType::Http);
        assert_eq!(info.center_frequency, 2.45e9);
        assert_eq!(info.span_frequency, 20e6);
        assert_eq!(info.bandwidth_hz, 16e6);
        assert!(info.bandwidth_hz <= info.span_frequency);
        assert_eq!(info.reference_level, -20.0);
        assert_eq!(info.device_serial, Some("SERIAL123".to_string()));
    }

    #[test]
    fn test_source_info_display() {
        // Test SourceInfo display formatting
        let info = SourceInfo {
            source_type: SourceType::File,
            center_frequency: 915e6,
            span_frequency: 10e6,
            bandwidth_hz: 8e6,
            reference_level: -30.0,
            device_serial: None,
        };

        let display_str = format!("{}", info);
        assert!(display_str.contains("File"));
        assert!(display_str.contains("915.0 MHz"));
        assert!(display_str.contains("10.0 MHz")); // sample rate
        assert!(display_str.contains("8.000 MHz RX-BW")); // distinct RX bandwidth
        assert!(display_str.contains("-30.0 dBm"));

        // When the RX bandwidth is unknown (0.0) it is omitted entirely.
        let unknown_bw = SourceInfo {
            bandwidth_hz: 0.0,
            ..info
        };
        assert!(!format!("{}", unknown_bw).contains("RX-BW"));
    }

    #[tokio::test]
    async fn test_detect_best_source_type_file_exists() {
        // Test file detection when file exists
        let temp_file = NamedTempFile::new().expect("Should create temp file");
        let config = AaroniaConfig::from_file(temp_file.path());

        // Create a minimal AaroniaSource for testing detect_best_source_type
        let source = AaroniaSource {
            config,
            source_type: SourceType::Http, // Will be updated
            sample_buffer: VecDeque::new(),
            #[cfg(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            ))]
            native_source: None,
            http_client: None,
            file_source: None,
            http_receiver: None,
            http_task: None,
            pending_overrun: false,
        };

        let detected_type = source
            .detect_best_source_type()
            .await
            .expect("Should detect source type");
        assert_eq!(detected_type, SourceType::File);
    }

    #[tokio::test]
    async fn file_source_propagates_center_and_rate_from_metadata() {
        // Regression test for the center-frequency wiring: the file
        // source must surface the RTSA metadata's tuning through
        // `get_source_info()`, overriding the builder's placeholder
        // defaults (2.44 GHz center / 15.36 MHz span). The CW fixture is
        // tuned to 2410 MHz at 1 MSPS — *both* differ from the defaults,
        // so passing assertions prove the file values win rather than a
        // coincidental match. Before the fix, `init_file_source` only
        // propagated the sample rate, so the center stayed at 0.0 / the
        // default and every emitted packet was tagged at the wrong RF
        // frequency.
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/IQ-Sample-Data-CW-2410MHz-1MHzSampleRate.rtsa"
        );
        if std::fs::metadata(fixture).is_ok_and(|meta| meta.len() == 132) {
            println!("Skipping test: LFS fixture missing");
            return;
        }
        let mut builder = AaroniaSourceBuilder::new();
        builder.file_source(fixture);

        let source = match builder.build().await {
            Ok(s) => s,
            Err(e) if e.to_string().contains("RTSAFileTool was not found") => {
                println!("Skipping test: RTSAFileTool not found to decompress CW fixture");
                return;
            }
            Err(e) => panic!("CW fixture should open: {}", e),
        };

        let info = source.get_source_info();
        assert_eq!(info.source_type, SourceType::File);
        assert!(
            (info.center_frequency - 2_410_000_000.0).abs() < 1_000.0,
            "expected ~2410 MHz center from file metadata, got {} Hz (default is 2.44 GHz)",
            info.center_frequency
        );
        assert!(
            (info.span_frequency - 1_000_000.0).abs() < 1_000.0,
            "expected ~1 MHz span from file metadata, got {} Hz (default is 15.36 MHz)",
            info.span_frequency
        );
    }

    #[tokio::test]
    async fn test_detect_best_source_type_file_not_found() {
        // Test file detection when file doesn't exist
        let config = AaroniaConfig::from_file("/nonexistent/path/test.rtsa");

        let source = AaroniaSource {
            config,
            source_type: SourceType::Http,
            sample_buffer: VecDeque::new(),
            #[cfg(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            ))]
            native_source: None,
            http_client: None,
            file_source: None,
            http_receiver: None,
            http_task: None,
            pending_overrun: false,
        };

        let result = source.detect_best_source_type().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("file not found"));
    }

    #[tokio::test]
    async fn test_detect_best_source_type_http() {
        // Test HTTP detection when URL provided
        let config = AaroniaConfig::from_http("http://rtsa-device:54664");

        let source = AaroniaSource {
            config,
            source_type: SourceType::File,
            sample_buffer: VecDeque::new(),
            #[cfg(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            ))]
            native_source: None,
            http_client: None,
            file_source: None,
            http_receiver: None,
            http_task: None,
            pending_overrun: false,
        };

        let detected_type = source
            .detect_best_source_type()
            .await
            .expect("Should detect source type");
        assert_eq!(detected_type, SourceType::Http);
    }

    #[tokio::test]
    async fn test_detect_best_source_type_localhost_fallback() {
        // Test localhost HTTP fallback when no specific source provided
        let config = AaroniaConfig::default();

        let source = AaroniaSource {
            config,
            source_type: SourceType::NativeSdk,
            sample_buffer: VecDeque::new(),
            #[cfg(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            ))]
            native_source: None,
            http_client: None,
            file_source: None,
            http_receiver: None,
            http_task: None,
            pending_overrun: false,
        };

        let detected_type = source
            .detect_best_source_type()
            .await
            .expect("Should detect source type");
        // Should fallback to HTTP (since SDK detection is complex in unit tests)
        assert_eq!(detected_type, SourceType::Http);
    }

    #[test]
    fn test_complex_configuration_scenarios() {
        // Test various configuration scenarios
        let config1 = AaroniaConfig::default()
            .center_frequency(2.4e9)
            .span_frequency(40e6)
            .reference_level(-10.0);

        assert_eq!(config1.center_frequency, 2.4e9);
        assert_eq!(config1.span_frequency, 40e6);
        assert_eq!(config1.reference_level, -10.0);

        // Test overriding default values
        let config2 = AaroniaConfig::from_http("http://localhost:8080")
            .center_frequency(915e6)
            .device_serial("OVERRIDE123".to_string());

        assert_eq!(config2.center_frequency, 915e6);
        assert_eq!(config2.device_serial, Some("OVERRIDE123".to_string()));
        assert_eq!(config2.force_source_type, Some(SourceType::Http));
    }

    #[test]
    fn test_edge_case_configurations() {
        // Test edge cases and boundary conditions
        let config = AaroniaConfig::default()
            .center_frequency(0.0)
            .span_frequency(-1.0)
            .reference_level(100.0);

        assert_eq!(config.center_frequency, 0.0);
        assert_eq!(config.span_frequency, -1.0);
        assert_eq!(config.reference_level, 100.0);

        // Test empty strings
        let config_empty = AaroniaConfig::from_http("").device_serial("".to_string());

        assert_eq!(config_empty.http_base_url, Some("".to_string()));
        assert_eq!(config_empty.device_serial, Some("".to_string()));
    }

    #[test]
    fn test_configuration_chaining() {
        // Test method chaining for configuration
        let config = AaroniaConfig::default()
            .center_frequency(1e9)
            .span_frequency(10e6)
            .reference_level(-40.0)
            .device_serial("CHAIN123".to_string())
            .force_native_sdk();

        assert_eq!(config.center_frequency, 1e9);
        assert_eq!(config.span_frequency, 10e6);
        assert_eq!(config.reference_level, -40.0);
        assert_eq!(config.device_serial, Some("CHAIN123".to_string()));
        assert_eq!(config.force_source_type, Some(SourceType::NativeSdk));
    }

    #[test]
    fn test_builder_immutable_vs_mutable() {
        // Test builder pattern both mutable and return-based approaches
        let mut builder = AaroniaSourceBuilder::new();
        builder.center_frequency(2.4e9);
        builder.span_frequency(20e6);

        assert_eq!(builder.config.center_frequency, 2.4e9);
        assert_eq!(builder.config.span_frequency, 20e6);

        // Test chaining
        let mut builder2 = AaroniaSourceBuilder::new();
        builder2.center_frequency(915e6);
        assert_eq!(builder2.config.center_frequency, 915e6);
    }

    #[test]
    fn test_pathbuf_file_source() {
        // Test PathBuf compatibility for file sources
        let path = PathBuf::from("/tmp/test.rtsa");
        let mut builder = AaroniaSourceBuilder::new();
        builder.file_source(&path);

        assert_eq!(builder.config.file_path, Some("/tmp/test.rtsa".to_string()));

        // Test with AaroniaConfig::from_file
        let config = AaroniaConfig::from_file(&path);
        assert_eq!(config.file_path, Some("/tmp/test.rtsa".to_string()));
    }

    #[test]
    fn test_source_type_priority_logic() {
        // Test the documented priority logic in configuration

        // Priority 1: File source
        let file_config = AaroniaConfig::from_file("/test.rtsa");
        assert_eq!(file_config.force_source_type, Some(SourceType::File));

        // Priority 2: HTTP source
        let http_config = AaroniaConfig::from_http("http://localhost:54664");
        assert_eq!(http_config.force_source_type, Some(SourceType::Http));

        // Priority 3: Native SDK (manual force)
        let sdk_config = AaroniaConfig::default().force_native_sdk();
        assert_eq!(sdk_config.force_source_type, Some(SourceType::NativeSdk));
    }

    #[test]
    fn test_configuration_validation_boundaries() {
        // Test various frequency and power level boundaries
        let extreme_config = AaroniaConfig::default()
            .center_frequency(f64::MAX)
            .span_frequency(f64::MIN)
            .reference_level(f64::INFINITY);

        assert_eq!(extreme_config.center_frequency, f64::MAX);
        assert_eq!(extreme_config.span_frequency, f64::MIN);
        assert!(extreme_config.reference_level.is_infinite());

        // Test NaN values
        let nan_config = AaroniaConfig::default().center_frequency(f64::NAN);
        assert!(nan_config.center_frequency.is_nan());
    }

    #[test]
    fn test_clone_and_debug_traits() {
        // Test Clone and Debug trait implementations
        let original_config = AaroniaConfig::from_http("http://test.com")
            .center_frequency(2.4e9)
            .device_serial("TEST123".to_string());

        let cloned_config = original_config.clone();
        assert_eq!(original_config.http_base_url, cloned_config.http_base_url);
        assert_eq!(
            original_config.center_frequency,
            cloned_config.center_frequency
        );
        assert_eq!(original_config.device_serial, cloned_config.device_serial);

        // Test Debug formatting
        let debug_str = format!("{:?}", original_config);
        assert!(debug_str.contains("http://test.com"));
        assert!(debug_str.contains("TEST123"));
    }

    #[tokio::test]
    async fn take_overrun_reflects_a_dropped_http_chunk() {
        // Regression test for overrun wiring: a chunk arriving from the
        // HTTP reader task flagged as dropped must surface via
        // `take_overrun()` on the next call, and clear after being read.
        let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel(4);
        let mut source = AaroniaSource {
            config: AaroniaConfig::default(),
            source_type: SourceType::Http,
            sample_buffer: VecDeque::new(),
            #[cfg(all(
                feature = "native-sdk",
                any(target_os = "windows", target_os = "linux")
            ))]
            native_source: None,
            http_client: None,
            file_source: None,
            http_receiver: Some(chunk_rx),
            http_task: None,
            pending_overrun: false,
        };

        assert!(!source.take_overrun(), "no chunk received yet");

        chunk_tx
            .send((vec![Complex32::new(1.0, 0.0)], false))
            .await
            .expect("channel open");
        chunk_tx
            .send((vec![Complex32::new(2.0, 0.0)], true))
            .await
            .expect("channel open");
        drop(chunk_tx);

        let mut buffer = Vec::new();
        let n = source
            .read_samples(&mut buffer, 2)
            .await
            .expect("read_samples should drain both queued chunks");
        assert_eq!(n, 2);

        assert!(
            source.take_overrun(),
            "a dropped chunk was received during read_samples"
        );
        assert!(
            !source.take_overrun(),
            "take_overrun must clear the flag after reading it"
        );
    }
}
