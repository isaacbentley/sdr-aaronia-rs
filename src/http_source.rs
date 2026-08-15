use crate::{Error, Result};
use futuresdr::prelude::*;

use num_complex::Complex32;

use bytes::Bytes;
use std::collections::VecDeque;
use tracing::{debug, info, trace, warn};

// Import our new advanced streaming capabilities
use crate::http_endpoints::{AuthMethod, HttpEndpointsClient};
use crate::http_streaming::{DropDetector, StreamFormat, StreamParser};

/// How many of the largest observed packets the sample buffer must be able to
/// hold, whatever `buffer_size` the caller configured.
///
/// One packet is the correctness floor — below it, the per-packet trim
/// discards samples the consumer was never offered. The rest is headroom:
/// `work()` drains this buffer and the scheduler does not call it on a clock,
/// so a couple of packets' slack absorbs ordinary jitter without letting the
/// buffer grow unbounded (packet size is the device's choice and small — a
/// few tens of thousands of samples).
const PACKET_CAPACITY_FACTOR: usize = 4;

/// Free output-port space required before the source's `work()` runs.
///
/// Without a floor here the runtime called `work()` whenever the output had
/// *any* room — an average of 14 samples free, measured at 106,000 calls a
/// second, each copying a handful of samples and returning. That spin burned
/// a core and crowded the scheduler, so the downstream pipeline block was run
/// only ~26 times a second despite having data waiting. Gating on a useful
/// block turns thousands of near-empty copies into one substantial one.
const SOURCE_MIN_OUTPUT_SAMPLES: usize = 65536;

/// Most stream chunks one `work()` call will read before yielding.
///
/// `fetch_samples` reads exactly one chunk, so a single fetch per `work()`
/// capped throughput at `chunk_size x work_calls_per_second` regardless of the
/// link. Looping until the buffer is stocked decouples throughput from the
/// scheduler's call rate; this bounds the loop so a fast server cannot let one
/// call monopolise the executor. At ~40,000 samples a chunk this is well over
/// a full buffer, so the buffer's capacity stops the loop first in practice.
const MAX_FETCHES_PER_WORK: usize = 16;

/// Stream statistics for monitoring
#[derive(Debug, Clone)]
pub struct StreamStats {
    pub active: bool,
    pub format: StreamFormat,
    pub current_frequency: f64,
    pub current_sample_rate: f64,
    pub buffer_level: usize,
    pub buffer_capacity: usize,
    pub input_name: Option<String>,
    pub input_msps: f64,
    pub dropped_packets: u64,
    pub packet_rate: f64,
    pub restart_pending: bool,
}

impl Default for StreamStats {
    fn default() -> Self {
        Self {
            active: false,
            format: StreamFormat::Int16,
            current_frequency: 0.0,
            current_sample_rate: 0.0,
            buffer_level: 0,
            buffer_capacity: 0,
            input_name: None,
            input_msps: 0.0,
            dropped_packets: 0,
            packet_rate: 0.0,
            restart_pending: false,
        }
    }
}

/// `FutureSDR` integration block for advanced Aaronia HTTP streaming.
///
/// This block provides an adapter between the RTSA HTTP streaming protocol and
/// a `FutureSDR` flowgraph. For most direct asynchronous streaming use cases,
/// the core path is [`HttpEndpointsClient::start_stream`] instead of this block.
#[derive(Block)]
pub struct HttpSource {
    #[output]
    output: futuresdr::runtime::buffer::DefaultCpuWriter<Complex32>,
    // Connection configuration
    base_url: String,
    // Note: actual frequency and sample_rate come from stream metadata

    // Enhanced HTTP client with endpoint support
    endpoints_client: HttpEndpointsClient,
    streaming_client: reqwest::Client,

    // Internal buffer for samples
    sample_buffer: VecDeque<Complex32>,

    /// Largest packet the device has delivered so far, in samples.
    ///
    /// The capacity floor tracks this because the trim runs as each packet
    /// lands, inside the fetch loop. A capacity below one packet therefore
    /// guillotines every packet on arrival: samples are discarded before the
    /// consumer is ever offered them, which is not "the consumer can't keep
    /// up" — it is the buffer throwing away data nobody declined. Learned
    /// from the stream rather than configured, since the packet size is the
    /// device's choice and no caller can be expected to guess it.
    max_packet_samples: usize,

    // Advanced streaming configuration
    stream_format: StreamFormat,
    stream_parser: StreamParser,
    input_name: Option<String>,  // Selected input stream
    rate_reduction: Option<u32>, // Sample rate reduction factor
    /// Server-side `?scale=N` query parameter for `/stream` (v9 PDF).
    /// Independent of the per-packet `scale` carried in JSON metadata.
    scale: Option<f64>,

    // Stream state
    stream_active: bool,
    current_frequency: f64,
    current_sample_rate: f64,
    stream_response: Option<
        Box<dyn futures::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + Unpin>,
    >,

    // Configuration
    buffer_size: usize,
    /// Hardware reference level to push, or `None` to leave the device's
    /// current gain alone.
    ///
    /// This used to be a bare `f64` that was pushed unconditionally on every
    /// start, so simply launching the app overwrote whatever gain the operator
    /// had set — with the caller's default, not a value they had chosen.
    /// `None` is what "the user did not ask for a reference level" looks like,
    /// and `CaptureConfig` leaves `None` fields untouched.
    reference_level: Option<f64>,

    // Authentication
    auth_method: AuthMethod,
    tokio_handle: Option<tokio::runtime::Handle>,

    // Shared statistics and drop detection
    shared_stats: Option<std::sync::Arc<std::sync::RwLock<StreamStats>>>,
    drop_detector: DropDetector,

    /// Whether the one-shot device retune in [`Self::configure_rtsa_device`]
    /// has already run. The tune applies the builder's centre / span /
    /// reference level on the **first** stream start; subsequent restarts
    /// (triggered via `shared_stats.restart_pending` after an external
    /// retune) must only reconnect the `/stream`, never re-push this
    /// source's now-stale target, which would undo that retune.
    initial_tune_done: bool,
}

impl HttpSource {
    /// Build a FutureSDR `Block` wrapping an `HttpSource` with basic
    /// options; use [`HttpSourceBuilder`] or [`Self::with_advanced_options`]
    /// for finer control (auth, input selection, rate reduction, scale).
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        base_url: String,
        frequency: f64,
        sample_rate: f64,
        reference_level: Option<f64>,
        buffer_size: usize,
        timeout_ms: u64,
    ) -> Result<Self> {
        Self::with_advanced_options(
            base_url,
            frequency,
            sample_rate,
            reference_level,
            buffer_size,
            timeout_ms,
            StreamFormat::Float32, // Default to float32 for compatibility
            AuthMethod::None,
            None, // No specific input
            None, // No rate reduction
            None, // No server-side scale override
        )
    }

    /// Create HttpSource with advanced streaming options
    #[allow(clippy::too_many_arguments)]
    pub fn with_advanced_options(
        base_url: String,
        frequency: f64,
        sample_rate: f64,
        reference_level: Option<f64>,
        buffer_size: usize,
        timeout_ms: u64,
        stream_format: StreamFormat,
        auth_method: AuthMethod,
        input_name: Option<String>,
        rate_reduction: Option<u32>,
        scale: Option<f64>,
    ) -> Result<Self> {
        // Security: Validate and sanitize the base URL (preserve existing validation)
        let parsed_url = url::Url::parse(&base_url)
            .map_err(|_| Error::Protocol(format!("Invalid base URL format: {}", base_url)))?;

        // Security: Restrict to allowed schemes and hosts
        match parsed_url.scheme() {
            "http" | "https" => {}
            _ => {
                return Err(Error::Protocol(format!(
                    "Only HTTP/HTTPS URLs are allowed, got: {}",
                    parsed_url.scheme()
                )));
            }
        }

        // Security: Optional - restrict to local/trusted networks only
        if let Some(host) = parsed_url.host() {
            match host {
                url::Host::Ipv4(ip) => {
                    if !ip.is_loopback() && !ip.is_private() {
                        warn!("Connecting to public IP address {}", ip);
                    }
                }
                url::Host::Ipv6(ip) => {
                    if !ip.is_loopback() {
                        warn!("Connecting to IPv6 address {}", ip);
                    }
                }
                url::Host::Domain(domain) => {
                    if !domain.starts_with("localhost") && !domain.ends_with(".local") {
                        warn!("Connecting to external domain {}", domain);
                    }
                }
            }
        }

        // Create HTTP endpoints client for advanced control
        let endpoints_client = HttpEndpointsClient::new(base_url.clone(), auth_method.clone())?;

        // Create streaming client - no timeout for active streaming, only for connection establishment
        let streaming_client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_millis(timeout_ms.min(30000))) // Connection timeout only
            .user_agent(crate::utils::user_agent())
            .build()?;

        // Initialize stream parser for chosen format. The parser must know
        // the server-side `?scale=N` encode multiplier requested for the
        // stream, or int16 decoding falls back to the full-scale default
        // and every sample is off by a factor of 32768/N.
        let stream_parser = StreamParser::new(stream_format, scale)?;

        let tokio_handle = tokio::runtime::Handle::try_current().ok();

        // Do not run `work()` until there is a worthwhile block of room
        // downstream — see `SOURCE_MIN_OUTPUT_SAMPLES`. Set before the port is
        // connected, which is the only time it takes effect.
        let mut output: futuresdr::runtime::buffer::DefaultCpuWriter<Complex32> =
            Default::default();
        output.set_min_items(SOURCE_MIN_OUTPUT_SAMPLES);

        Ok(Self {
            output,
            base_url,
            endpoints_client,
            streaming_client,
            sample_buffer: VecDeque::with_capacity(buffer_size * 2),
            max_packet_samples: 0,
            stream_format,
            stream_parser,
            input_name,
            rate_reduction,
            scale,
            stream_active: false,
            current_frequency: frequency,
            current_sample_rate: sample_rate,
            stream_response: None, // Initialize to None
            buffer_size,
            reference_level,
            auth_method,
            tokio_handle,
            shared_stats: None,
            drop_detector: DropDetector::default(),
            initial_tune_done: false,
        })
    }

    async fn start_stream(&mut self) -> Result<()> {
        // The `native_client` field is still populated by the
        // `with_native_sdk(true)` builder method for API compatibility,
        // but the high-level convenience methods (`init`, `start_stream`,
        // `get_sample_rate`, `get_frequency`, `get_iq_samples`) that the
        // old fallback branch called against `NativeSdkClient` were
        // removed when the FFI surface was flattened to its current
        // low-level form. Routing HTTP streaming through the native SDK
        // now lives in `sdk_source.rs` and `unified_source.rs`; this
        // path always falls through to plain HTTP.
        info!("Initializing advanced Aaronia HTTP streaming");
        info!("Stream format: {}", self.stream_format.as_str());

        // Get server info to verify connection using Tokio runtime handle
        match self.endpoints_client.get_info().await {
            Ok(server_info) => {
                info!(
                    "Connected to RTSA server: {} ({})",
                    server_info.title, server_info.name
                );
                if !server_info.mission.is_empty() {
                    info!("Active mission: {}", server_info.mission);
                }
            }
            Err(e) => {
                warn!("Could not get server info (continuing anyway): {}", e);
            }
        }

        // Check available inputs if no specific input requested
        if self.input_name.is_none() {
            match self.endpoints_client.get_inputs().await {
                Ok(inputs) => {
                    if !inputs.is_empty() {
                        info!("Available inputs: {:?}", inputs);
                        // Use "main" input if available, otherwise first available
                        let selected = if inputs.contains(&"main".to_string()) {
                            "main".to_string()
                        } else {
                            inputs[0].clone()
                        };
                        info!("Selected input: {}", selected);
                        self.input_name = Some(selected);
                    }
                }
                Err(e) => {
                    debug!("Could not enumerate inputs: {}", e);
                }
            }
        }

        // Configure RTSA device to enable connection and streaming
        if let Err(e) = self.configure_rtsa_device().await {
            debug!("Could not configure RTSA device: {}", e);
        }

        // Try to start streaming via control endpoint
        match self.endpoints_client.control_streaming(true).await {
            Ok(_) => info!("Started streaming via control endpoint"),
            Err(e) => debug!(
                "Could not control streaming (device may already be streaming): {}",
                e
            ),
        }

        // Build streaming URL with advanced parameters. Values are
        // percent-encoded — input names come from the server and may
        // contain characters that would corrupt a hand-built query string.
        let stream_url = {
            // Scoped: the Serializer holds a non-Send trait object and must
            // drop before the `.await`s below.
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query.append_pair("format", self.stream_format.as_str());
            if let Some(ref input) = self.input_name {
                query.append_pair("input", input);
            }
            if let Some(reduction) = self.rate_reduction {
                query.append_pair("rate_reduction", &reduction.to_string());
            }
            // Server-side scale (?scale=N) for the int16 path, per v9 PDF.
            if let Some(scale) = self.scale {
                query.append_pair("scale", &scale.to_string());
            }
            format!("{}/stream?{}", self.base_url, query.finish())
        };

        info!("Constructed stream URL: {}", stream_url);

        // Apply authentication based on method
        let mut request_builder = self.streaming_client.get(&stream_url);
        request_builder = match &self.auth_method {
            AuthMethod::Basic { username, password } => {
                request_builder.basic_auth(username, Some(password))
            }
            AuthMethod::Token { token } => {
                request_builder.header("Authorization", format!("RToken {}", token))
            }
            AuthMethod::None => request_builder,
        };

        info!("Sending stream request to {}.", stream_url);
        info!("Sending HTTP request...");

        let response = request_builder.send().await?;
        info!("Received HTTP response status: {}", response.status());
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Protocol(format!(
                "Stream endpoint returned error: {}",
                status
            )));
        }

        // Store the byte stream
        self.stream_response = Some(Box::new(response.bytes_stream()));

        self.stream_active = true;
        info!("Advanced Aaronia HTTP streaming initialized");

        Ok(())
    }

    async fn fetch_samples(&mut self) -> Result<usize> {
        {
            debug!("fetch_samples() called");
        }
        trace!("Fetching samples from stream...");

        // Sibling note to `start_stream`: the native-SDK sample-fetch
        // branch was deleted when `NativeSdkClient`'s `get_iq_samples`
        // helper was replaced by the lower-level `to_sample_*` config
        // operations. Use `sdk_source.rs` for native-SDK streaming.

        // Read from the existing persistent stream
        if let Some(stream) = &mut self.stream_response {
            let bytes_result = async {
                use futures::StreamExt;

                // For continuous streaming, we don't want to timeout waiting for data
                // The stream should provide data continuously, so we wait indefinitely
                // Only timeout on connection establishment, not data reception
                let chunk_result = stream.next().await;

                match chunk_result {
                    Some(Ok(chunk)) => {
                        trace!("Received HTTP stream chunk: {} bytes", chunk.len());
                        Ok(chunk)
                    }
                    Some(Err(e)) => {
                        // Stream error occurred
                        warn!("Stream chunk error: {}", e);
                        Err(Error::Protocol(format!("Stream chunk error: {}", e)))
                    }
                    None => {
                        // The body finished: no more data is coming.
                        // Reported as StreamClosed rather than Protocol
                        // so a caller can tell it apart from a read
                        // that failed mid-stream, which is the arm
                        // directly above.
                        warn!("Stream ended unexpectedly");
                        Err(Error::StreamClosed("HTTP response body ended".to_string()))
                    }
                }
            }
            .await;

            match bytes_result {
                Ok(bytes) => {
                    let samples_added = self.process_advanced_stream_data(&bytes)?;
                    trace!("Added {} samples from stream", samples_added);
                    Ok(samples_added)
                }
                Err(e) => {
                    // Clean up the failed stream
                    self.cleanup_stream().await;
                    Err(e)
                }
            }
        } else {
            // No active stream - this shouldn't happen if initialize was called
            warn!("No active stream available for reading");
            Err(Error::Protocol("No active stream".to_string()))
        }
    }

    /// Clean up the stream when it fails or ends
    async fn cleanup_stream(&mut self) {
        // Stop streaming via control endpoint to prevent device from continuing to stream
        if self.stream_active {
            match self.endpoints_client.control_streaming(false).await {
                Ok(_) => info!("Stopped streaming via control endpoint"),
                Err(e) => debug!(
                    "Could not stop streaming via control endpoint (device may handle this automatically): {}",
                    e
                ),
            }
        }

        self.stream_response = None;
        self.stream_active = false;
    }

    fn process_advanced_stream_data(&mut self, data: &Bytes) -> Result<usize> {
        // All stream formats use JSON+binary format
        // even binary formats like float16/float32/int16 when streaming via /stream endpoint
        // This means JSON metadata followed by record separator (ASCII 30) then binary data

        // Only build the (allocating) hex/ASCII preview when DEBUG logging
        // is actually enabled — this runs on every chunk of the hot path.
        if !data.is_empty() && tracing::enabled!(tracing::Level::DEBUG) {
            let preview_len = std::cmp::min(100, data.len());
            let preview_bytes: Vec<u8> = data[..preview_len].to_vec();
            let preview_ascii: String = preview_bytes
                .iter()
                .map(|&b| {
                    if (32..=126).contains(&b) {
                        b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            debug!(
                "Data preview: {}B | First 20 bytes: {:?} | ASCII: {}",
                data.len(),
                &preview_bytes[..std::cmp::min(20, preview_len)],
                &preview_ascii[..std::cmp::min(50, preview_len)]
            );
        }

        let packets = self.stream_parser.process_data(data)?;

        let mut total_samples_added = 0;

        for packet in &packets {
            let _ = self.drop_detector.observe(packet);
        }

        for packet in packets {
            // Update current stream metadata from the parsed packet. The
            // packet reports its frequency *range*; the tuned frequency is
            // the center of that range, not its lower edge.
            self.current_frequency = packet.sdr_config.center_frequency;

            // The parser derives the rate from `sampleFrequency` when
            // present and from `samples / duration` otherwise; adopt it
            // when it differs meaningfully from what we believe.
            let inferred_rate = packet.sdr_config.sample_rate;
            if inferred_rate > 0.0
                && (self.current_sample_rate <= 0.0
                    || (inferred_rate - self.current_sample_rate).abs() / self.current_sample_rate
                        > 0.1)
            {
                debug!(
                    "Sample rate updated from metadata: {:.0} -> {:.0} Hz",
                    self.current_sample_rate, inferred_rate
                );
                self.current_sample_rate = inferred_rate;
            }

            // Add samples to the buffer, enforcing the configured capacity:
            // if the consumer can't keep up, drop the *oldest* samples so
            // the buffer stays bounded and current.
            let packet_samples = packet.samples.len();
            // Raise the floor *before* trimming, so the packet that widens
            // it is itself protected rather than being the last casualty.
            self.max_packet_samples = self.max_packet_samples.max(packet_samples);
            // Bulk `extend` (one reserve) rather than per-element `push_back`.
            self.sample_buffer.extend(packet.samples);
            total_samples_added += packet_samples;
            let capacity = self.buffer_capacity();
            if self.sample_buffer.len() > capacity {
                let overflow = self.sample_buffer.len() - capacity;
                self.sample_buffer.drain(0..overflow);
                warn!(
                    "Sample buffer overflow: dropped {} oldest samples (capacity {})",
                    overflow, capacity
                );
            }

            if packet_samples > 0 {
                trace!(
                    "Added {} samples to buffer from packet (payload: {:?})",
                    packet_samples, packet.metadata.payload
                );
            }
        }

        if let Some(ref shared) = self.shared_stats
            && let Ok(mut stats) = shared.write()
        {
            let pending = stats.restart_pending;
            *stats = self.get_stream_stats();
            stats.restart_pending = pending;
        }

        Ok(total_samples_added)
    }

    /// Hard cap on `sample_buffer`, above which the oldest samples are
    /// dropped to keep the buffer bounded and current.
    ///
    /// Single source of truth for both the enforcement in
    /// [`Self::fetch_samples`] and the figure reported by
    /// [`Self::get_stream_stats`]; these were separately-written
    /// expressions (`saturating_mul(2).max(1)` vs a bare `* 2`) that
    /// disagreed whenever `buffer_size` was 0 — reporting capacity 0 while
    /// enforcing 1, so `buffer_level` could exceed `buffer_capacity` and a
    /// consumer computing a fill ratio divided by zero. The bare `* 2` was
    /// also the only unguarded one on overflow.
    fn buffer_capacity(&self) -> usize {
        let configured = self.buffer_size.saturating_mul(2).max(1);
        // Never sit below a few packets. See `max_packet_samples`: the trim
        // runs per packet, so a capacity under one packet discards most of
        // every packet the moment it arrives, no matter how fast the
        // consumer is. The factor buys headroom for scheduler jitter, since
        // `work()` is what drains this and it is not called on a clock.
        let packet_floor = self
            .max_packet_samples
            .saturating_mul(PACKET_CAPACITY_FACTOR);
        configured.max(packet_floor)
    }

    /// Get current stream statistics for monitoring
    pub fn get_stream_stats(&self) -> StreamStats {
        let stats = self.stream_parser.stats();
        StreamStats {
            active: self.stream_active,
            format: self.stream_format,
            current_frequency: self.current_frequency,
            current_sample_rate: self.current_sample_rate,
            buffer_level: self.sample_buffer.len(),
            buffer_capacity: self.buffer_capacity(),
            input_name: self.input_name.clone(),
            input_msps: stats.samples_per_second / 1e6,
            dropped_packets: self.drop_detector.drops(),
            packet_rate: stats.packet_rate,
            restart_pending: false,
        }
    }
}

impl Drop for HttpSource {
    fn drop(&mut self) {
        // Ensure streaming is stopped when the source is dropped
        if self.stream_active {
            info!("Dropping HttpSource - stopping stream");
            // Don't use block_on in destructor as it can cause panics during runtime shutdown
            // Just mark as inactive and let the response drop naturally
            self.stream_response = None;
            self.stream_active = false;
            debug!("HttpSource cleanup completed without blocking");
        }
    }
}

impl HttpSource {
    /// Configure RTSA device to enable connection and streaming
    async fn configure_rtsa_device(&mut self) -> Result<()> {
        use crate::http_endpoints::ConfigItem;

        info!("Configuring RTSA device for streaming...");

        // Get current configuration to understand request structure
        let config = self.endpoints_client.get_config().await?;
        info!(
            "Retrieved device configuration, request ID: {}",
            config.request
        );

        // Configure RTSA block with connect=true and run=true
        let rtsa_config = vec![
            ConfigItem::Bool {
                name: "connect".to_string(),
                label: "Connect".to_string(),
                flags: String::new(),
                value: true,
                default: false,
                text_off: None,
                text_on: None,
            },
            ConfigItem::Bool {
                name: "run".to_string(),
                label: "Run".to_string(),
                flags: String::new(),
                value: true,
                default: false,
                text_off: None,
                text_on: None,
            },
        ];

        // Update RTSA configuration
        match self
            .endpoints_client
            .update_config(config.request + 1, "RTSA", rtsa_config)
            .await
        {
            Ok(_) => {
                info!("Successfully configured RTSA device: connect=true, run=true");
                // Give device a moment to process the configuration
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;

                // Tune the hardware *before* opening the stream — but only on
                // the first start. A restart (from `restart_pending`) happens
                // *after* an external retune, and re-pushing this source's
                // stale target would undo it; the stream just needs to
                // reconnect. See `initial_tune_done`.
                if self.initial_tune_done {
                    debug!("Skipping device retune on stream restart (already tuned once)");
                } else {
                    self.apply_initial_tune().await;
                    self.initial_tune_done = true;
                }

                Ok(())
            }
            Err(e) => {
                debug!("Could not update RTSA configuration: {}", e);
                Err(e)
            }
        }
    }

    /// Push this source's centre / span / reference level to the device and
    /// report what the device actually adopted.
    ///
    /// This replaced a `configure_capture` call on the `/control` endpoint,
    /// which "always succeeds": it logged a successful tune while the device
    /// never moved, because that endpoint does not carry these fields on a
    /// Spectran V6. The write now goes through `/remoteconfig` against the
    /// block that genuinely carries `centerfreq0`, and is read back — a wrong
    /// receiver name returns HTTP 200 and changes nothing, so the status code
    /// alone proves nothing.
    ///
    /// Span is expressed as the `decimation0` enum index rather than a
    /// frequency: the device has no span field, only a decimation ladder.
    ///
    /// A failure here is logged, not propagated. The stream is still worth
    /// opening on whatever the device is currently tuned to, and the caller
    /// gets a warning naming what did not take rather than a silent success.
    async fn apply_initial_tune(&mut self) {
        let decimation_index = (self.current_sample_rate > 0.0)
            .then(|| crate::decimation_index_for_rate(self.current_sample_rate));

        info!(
            "Tuning RTSA device to center={:.6} MHz, span={:.3} MHz \
             (decimation index {:?}), ref_level={:?} dBm",
            self.current_frequency / 1e6,
            self.current_sample_rate / 1e6,
            decimation_index,
            self.reference_level,
        );

        let request = crate::http_endpoints::CaptureConfig {
            center_freq_hz: (self.current_frequency > 0.0).then_some(self.current_frequency),
            decimation_index,
            reflevel_dbm: self.reference_level,
        };

        match self.endpoints_client.apply_capture_config(&request).await {
            Ok(applied) => {
                info!(
                    "RTSA device reports center={:?} Hz, decimation={:?}, \
                     ref_level={:?} dBm (block {})",
                    applied.center_freq_hz,
                    applied.decimation_index,
                    applied.reflevel_dbm,
                    applied.receiver_name,
                );

                // Compare against what was asked for. Tuning is snapped by the
                // hardware, so an exact match is not required — but a value
                // that did not move at all means the write was accepted and
                // discarded, which is the failure this read-back exists to
                // catch.
                if let (Some(want), Some(got)) = (request.center_freq_hz, applied.center_freq_hz)
                    && (want - got).abs() > 1e3
                {
                    warn!(
                        "Centre frequency did not take: asked {:.6} MHz, device reports {:.6} MHz",
                        want / 1e6,
                        got / 1e6
                    );
                }
                if let (Some(want), Some(got)) =
                    (request.decimation_index, applied.decimation_index)
                    && want != got
                {
                    warn!("Span did not take: asked decimation index {want}, device reports {got}");
                }
            }
            Err(e) => {
                warn!(
                    "Could not tune RTSA device via /remoteconfig: {e}. \
                     Streaming from whatever the device is currently tuned to."
                );
            }
        }
    }
}

impl Kernel for HttpSource {
    async fn init(
        &mut self,
        _mio: &mut futuresdr::runtime::MessageOutputs,
        _meta: &mut futuresdr::runtime::BlockMeta,
    ) -> anyhow::Result<()> {
        info!("HttpSource: INITIALIZED - Starting HTTP stream connection");
        let handle = self.tokio_handle.clone();
        if let Some(handle) = handle {
            handle.block_on(async { self.start_stream().await })?;
        } else {
            self.start_stream().await?;
        }
        info!("HttpSource: HTTP streaming connection established successfully");
        Ok(())
    }

    async fn work(
        &mut self,
        io: &mut futuresdr::runtime::WorkIo,
        _mio: &mut futuresdr::runtime::MessageOutputs,
        _meta: &mut futuresdr::runtime::BlockMeta,
    ) -> anyhow::Result<()> {
        let mut restart_triggered = false;
        if let Some(ref shared) = self.shared_stats
            && let Ok(mut stats) = shared.write()
            && stats.restart_pending
        {
            stats.restart_pending = false;
            restart_triggered = true;
        }

        if restart_triggered {
            info!("Restarting HTTP stream connection to apply frequency/span configuration...");
            let handle = self.tokio_handle.clone();
            if let Some(h) = handle {
                h.block_on(async {
                    self.cleanup_stream().await;
                    self.stream_active = false;
                    self.sample_buffer.clear();
                    if let Err(e) = self.start_stream().await {
                        warn!(
                            "Failed to restart stream during configuration change: {}",
                            e
                        );
                    }
                });
            } else {
                self.cleanup_stream().await;
                self.stream_active = false;
                self.sample_buffer.clear();
                if let Err(e) = self.start_stream().await {
                    warn!(
                        "Failed to restart stream during configuration change: {}",
                        e
                    );
                }
            }
        }

        {
            debug!(
                "HttpSource: work() called, buffer: {} samples",
                self.sample_buffer.len()
            );
        }
        let o_len = self.output.slice().len();

        // Keep the buffer stocked rather than topping it up only when it is
        // about to run dry. This was `if self.sample_buffer.len() < o_len`,
        // where `o_len` is the free output space — measured at ~14 samples —
        // so the source refilled only when nearly empty, then dribbled a
        // 40,000-sample chunk out a few samples at a time. Between refills the
        // HTTP socket was not read at all and the server's stream backed up.
        //
        // Each `fetch_samples` reads one chunk, so looping until the buffer is
        // half full pulls several chunks per call and keeps a read
        // outstanding. `curl` on the same endpoint sustains ~57 MB/s; this
        // path managed 5.8. (61.44 MS/s stays out of reach regardless: at
        // 4 bytes a sample that is 2 Gbit/s. The transport ceiling is the
        // link's ~57 MB/s, roughly 14 MS/s.)
        let refill_below = (self.buffer_capacity() / 2).max(o_len);
        let mut fetches = 0usize;
        while self.sample_buffer.len() < refill_below && fetches < MAX_FETCHES_PER_WORK {
            fetches += 1;
            let handle = self.tokio_handle.clone();
            let fetch_res = if let Some(handle) = handle {
                handle.block_on(async { self.fetch_samples().await })
            } else {
                self.fetch_samples().await
            };

            match fetch_res {
                Ok(fetched) => {
                    if fetched == 0 {
                        // No samples available, sleep briefly to prevent busy-waiting
                        // Use futures-timer for runtime-agnostic async sleep
                        futures_timer::Delay::new(std::time::Duration::from_millis(50)).await;
                        // Don't try to reconfigure - RTSA Suite handles device config
                    } else {
                        // Print stream info periodically when receiving data
                        // self.print_stream_info(); // moved to trace
                    }
                }
                Err(e) => {
                    warn!("Aaronia stream error: {}", e);
                    self.stream_active = false;
                    // Try to reconnect after a delay
                    futures_timer::Delay::new(std::time::Duration::from_millis(1000)).await;
                    let handle = self.tokio_handle.clone();
                    let reconnect_res = if let Some(handle) = handle {
                        handle.block_on(async { self.start_stream().await })
                    } else {
                        self.start_stream().await
                    };
                    if let Err(reconnect_err) = reconnect_res {
                        warn!("Failed to reconnect: {}", reconnect_err);
                    }
                    // Ask to be polled again so the (re)connected stream is
                    // drained promptly instead of waiting on downstream demand.
                    io.call_again = true;
                    return Ok(()); // Don't fail the entire flowgraph
                }
            }
        }

        let o = self.output.slice();
        // Copy available samples to output
        let samples_to_copy = std::cmp::min(self.sample_buffer.len(), o.len());
        for sample in o.iter_mut().take(samples_to_copy) {
            // Safe unwrap: samples_to_copy is limited by buffer length
            *sample = self
                .sample_buffer
                .pop_front()
                .expect("Buffer length verified");
        }

        self.output.produce(samples_to_copy);

        // Log sample production periodically
        if samples_to_copy > 0 {
            {
                debug!(
                    "HttpSource: Produced samples in this batch: {}",
                    samples_to_copy
                );
            }
        }

        // Request to be called again
        io.call_again = true;

        Ok(())
    }
}

/// Builder for the `FutureSDR` [`HttpSource`] block.
/// **HTTP Streaming vs Configuration**:
/// - **Basic HTTP Streaming**: Available without additional licensing
/// - **Device Configuration**: Requires separate "Remote Config" license from Aaronia
///
/// **Configuration Options**:
/// - **With Remote Config License**: Use `/remoteconfig` endpoint for real-time parameter changes
/// - **Without License**: Configuration parameters serve as initial/default values for streaming
/// - **Alternative**: Use Native SDK for configuration without HTTP licensing restrictions
///
/// See: <https://aaronia.com/en/software-licence-remote-config>
pub struct HttpSourceBuilder {
    base_url: String,
    frequency: f64,
    sample_rate: f64,
    reference_level: Option<f64>,
    buffer_size: usize,
    timeout_ms: u64,
    stream_format: StreamFormat,
    auth_method: AuthMethod,
    input_name: Option<String>,
    rate_reduction: Option<u32>,
    /// Server-side scale factor for the `?scale=N` query parameter on
    /// `/stream`. Per the v9 RTSA HTTP Stream Server Endpoints document,
    /// this scales the integer payload into a "meaningful numeric range"
    /// before transmission and is independent of the per-packet `scale`
    /// JSON field.
    scale: Option<f64>,
    shared_stats: Option<std::sync::Arc<std::sync::RwLock<StreamStats>>>,
}

impl HttpSourceBuilder {
    /// Create a builder targeting `base_url` with the standard defaults
    /// (100 MHz / 1 MS/s / Int16 / no auth).
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.to_string(),
            frequency: 100e6, // 100 MHz default
            sample_rate: 1e6, // 1 MS/s default
            // `None`, not a number: pushing a default reference level on
            // every start silently overwrites the operator's gain.
            reference_level: None,
            buffer_size: 4096,                  // 4k samples default
            timeout_ms: 15000,                  // 15s timeout default
            stream_format: StreamFormat::Int16, // Production default based on reference implementation
            auth_method: AuthMethod::None,      // No auth by default
            input_name: None,                   // Auto-select input
            rate_reduction: None,
            scale: None,
            shared_stats: None,
        }
    }

    /// Set the initial center frequency, in Hz.
    #[must_use]
    pub fn frequency(mut self, freq: f64) -> Self {
        self.frequency = freq;
        self
    }

    /// Set frequency from string with units (e.g., "146.52M", "2.4G", "162.5k")
    pub fn frequency_str(mut self, freq_str: &str) -> Result<Self> {
        self.frequency = crate::utils::parse_frequency(freq_str)?;
        Ok(self)
    }

    /// Set the initial sample rate, in Hz.
    #[must_use]
    pub fn sample_rate(mut self, rate: f64) -> Self {
        self.sample_rate = rate;
        self
    }

    /// Set sample rate from string with units (e.g., "25M", "10k", "2.5M")
    pub fn sample_rate_str(mut self, rate_str: &str) -> Result<Self> {
        self.sample_rate = crate::utils::parse_sample_rate(rate_str)?;
        Ok(self)
    }

    /// Set the initial reference level, in dBm.
    #[must_use]
    pub fn reference_level(mut self, level: f64) -> Self {
        self.reference_level = Some(level);
        self
    }

    /// Set the internal sample buffer capacity, in samples.
    #[must_use]
    pub fn buffer_size(mut self, size: usize) -> Self {
        self.buffer_size = size;
        self
    }

    /// Set the connection-establishment timeout, in milliseconds.
    #[must_use]
    pub fn timeout_ms(mut self, timeout: u64) -> Self {
        self.timeout_ms = timeout;
        self
    }

    /// Set streaming format (json, int16, float16, float32)
    #[must_use]
    pub fn format(mut self, format: StreamFormat) -> Self {
        self.stream_format = format;
        self
    }

    /// Set authentication method
    #[must_use]
    pub fn auth(mut self, auth: AuthMethod) -> Self {
        self.auth_method = auth;
        self
    }

    /// Set specific input stream name
    #[must_use]
    pub fn input(mut self, input_name: &str) -> Self {
        self.input_name = Some(input_name.to_string());
        self
    }

    /// Set rate reduction factor
    #[must_use]
    pub fn rate_reduction(mut self, factor: u32) -> Self {
        self.rate_reduction = Some(factor);
        self
    }

    /// Set the server-side `?scale=N` query parameter for `/stream`. Per the
    /// v9 RTSA HTTP Stream Server Endpoints document, this scales the
    /// integer payload before transmission (e.g.
    /// `/stream?format=int16&scale=1000000`) and is independent of the
    /// per-packet `scale` field carried in each JSON metadata header.
    #[must_use]
    pub fn scale(mut self, scale: f64) -> Self {
        self.scale = Some(scale);
        self
    }

    /// Share a `StreamStats` handle with an external caller.
    ///
    /// The built `HttpSource` refreshes this handle with its current
    /// [`StreamStats`] after every processed chunk (see
    /// `process_advanced_stream_data`), so callers can observe live
    /// throughput/format/frequency state without polling the block
    /// directly. It is also the write side of the retune mechanism: an
    /// external caller sets `restart_pending = true` on the shared
    /// `StreamStats`, and the next `work()` call notices it, tears down
    /// the current `/stream` connection, and reconnects to pick up
    /// whatever frequency/span configuration was applied in between.
    #[must_use]
    pub fn with_shared_stats(
        mut self,
        stats: std::sync::Arc<std::sync::RwLock<StreamStats>>,
    ) -> Self {
        self.shared_stats = Some(stats);
        self
    }

    /// No-op kept for backward API compatibility. `HttpSource` always
    /// streams over HTTP; routing through the native SDK instead lives in
    /// [`crate::sdk_source`] / [`crate::unified_source`].
    #[must_use]
    pub fn with_native_sdk(self, _enable: bool) -> Self {
        self
    }

    /// Build with basic options (backward compatibility)
    pub fn build(self) -> Result<HttpSource> {
        let mut source = HttpSource::with_advanced_options(
            self.base_url,
            self.frequency,
            self.sample_rate,
            self.reference_level,
            self.buffer_size,
            self.timeout_ms,
            self.stream_format,
            self.auth_method,
            self.input_name,
            self.rate_reduction,
            self.scale,
        )?;
        source.shared_stats = self.shared_stats;
        Ok(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_endpoints::AuthMethod;
    use crate::http_streaming::StreamFormat;

    // Test HttpSource creation and initialization
    #[tokio::test]
    async fn test_http_source_creation() {
        // Basic source creation should work
        let source = HttpSourceBuilder::new("http://localhost:54664")
            .frequency(146.52e6)
            .sample_rate(2.048e6)
            .format(StreamFormat::Float32)
            .buffer_size(4096)
            .timeout_ms(5000);

        // Should be able to build without errors for valid URL
        assert!(source.build().is_ok());
    }

    #[test]
    fn test_http_source_invalid_url() {
        // Invalid URL should fail validation
        let source = HttpSourceBuilder::new("invalid://bad-url");
        assert!(source.build().is_err());

        // Non-HTTP schemes should be rejected
        let source = HttpSourceBuilder::new("ftp://example.com");
        assert!(source.build().is_err());

        let source = HttpSourceBuilder::new("file:///etc/passwd");
        assert!(source.build().is_err());
    }

    #[tokio::test]
    async fn test_http_source_builder_configuration() {
        let source = HttpSourceBuilder::new("http://localhost:54664")
            .frequency(915e6)
            .sample_rate(10e6)
            .reference_level(-20.0)
            .buffer_size(8192)
            .timeout_ms(30000)
            .format(StreamFormat::Int16)
            .auth(AuthMethod::Basic {
                username: "user".to_string(),
                password: "pass".to_string(),
            })
            .input("main")
            .rate_reduction(4);

        // All configurations should be valid
        assert!(source.build().is_ok());
    }

    #[tokio::test]
    async fn test_frequency_string_parsing() {
        // Test frequency parsing with units
        let source = HttpSourceBuilder::new("http://localhost:54664")
            .frequency_str("146.52M")
            .expect("Should parse MHz");
        assert!(source.build().is_ok());

        let source = HttpSourceBuilder::new("http://localhost:54664")
            .frequency_str("2.4G")
            .expect("Should parse GHz");
        assert!(source.build().is_ok());

        let source = HttpSourceBuilder::new("http://localhost:54664")
            .frequency_str("162.5k")
            .expect("Should parse kHz");
        assert!(source.build().is_ok());

        // Invalid frequency strings should fail
        let result = HttpSourceBuilder::new("http://localhost:54664").frequency_str("invalid");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_sample_rate_string_parsing() {
        // Test sample rate parsing with units
        let source = HttpSourceBuilder::new("http://localhost:54664")
            .sample_rate_str("25M")
            .expect("Should parse MHz");
        assert!(source.build().is_ok());

        let source = HttpSourceBuilder::new("http://localhost:54664")
            .sample_rate_str("2.048M")
            .expect("Should parse fractional MHz");
        assert!(source.build().is_ok());

        let source = HttpSourceBuilder::new("http://localhost:54664")
            .sample_rate_str("100k")
            .expect("Should parse kHz");
        assert!(source.build().is_ok());

        // Invalid sample rate strings should fail
        let result = HttpSourceBuilder::new("http://localhost:54664").sample_rate_str("invalid");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_stream_format_configuration() {
        // Test all supported stream formats
        for format in [
            StreamFormat::Json,
            StreamFormat::Int16,
            StreamFormat::Float16,
            StreamFormat::Float32,
        ] {
            let source = HttpSourceBuilder::new("http://localhost:54664").format(format);
            assert!(source.build().is_ok());
        }
    }

    #[tokio::test]
    async fn test_authentication_methods() {
        // Test no authentication
        let source = HttpSourceBuilder::new("http://localhost:54664").auth(AuthMethod::None);
        assert!(source.build().is_ok());

        // Test basic authentication
        let source = HttpSourceBuilder::new("http://localhost:54664").auth(AuthMethod::Basic {
            username: "testuser".to_string(),
            password: "testpass".to_string(),
        });
        assert!(source.build().is_ok());

        // Test token authentication
        let source = HttpSourceBuilder::new("http://localhost:54664").auth(AuthMethod::Token {
            token: "test-token-123".to_string(),
        });
        assert!(source.build().is_ok());
    }

    #[test]
    fn test_stream_statistics() {
        // This module lives inside `http_source.rs`, so it can read the
        // block's private fields directly — no need to stop at "the builder
        // returned Ok", which is all this test used to check.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let block = HttpSourceBuilder::new("http://localhost:54664")
            .frequency(915e6)
            .sample_rate(2.048e6)
            .format(StreamFormat::Float32)
            .input("main")
            .rate_reduction(2)
            .buffer_size(4096)
            .build()
            .expect("Should create HttpSource");

        let stats = block.get_stream_stats();
        assert!(!stats.active, "a freshly built source is not streaming yet");
        assert_eq!(stats.format, StreamFormat::Float32);
        assert_eq!(stats.current_frequency, 915e6);
        assert_eq!(stats.current_sample_rate, 2.048e6);
        assert_eq!(stats.input_name.as_deref(), Some("main"));
        assert_eq!(stats.buffer_level, 0, "nothing buffered before streaming");
        assert_eq!(stats.buffer_capacity, 8192, "2x the configured buffer_size");
        assert!(!stats.restart_pending);
    }

    /// `buffer_capacity` must equal the cap actually enforced when trimming
    /// `sample_buffer`, for every `buffer_size` the builder accepts —
    /// including 0, which `test_buffer_configuration` explicitly allows.
    ///
    /// These were two separately-written expressions:
    /// `saturating_mul(2).max(1)` at the enforcement site and a bare `* 2`
    /// in the reported stats. At `buffer_size: 0` they disagreed — capacity
    /// was reported as 0 while 1 was enforced, so `buffer_level` could
    /// exceed `buffer_capacity` and a consumer computing a fill ratio
    /// divided by zero.
    #[test]
    fn stream_stats_capacity_matches_the_enforced_cap() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        for buffer_size in [0usize, 1, 1024, 4096] {
            let block = HttpSourceBuilder::new("http://localhost:54664")
                .buffer_size(buffer_size)
                .build()
                .expect("Should create HttpSource");

            let reported = block.get_stream_stats().buffer_capacity;
            assert_eq!(
                reported,
                block.buffer_capacity(),
                "reported capacity must be the enforced one (buffer_size={buffer_size})"
            );
            assert!(
                reported > 0,
                "capacity must never be 0 — consumers divide by it (buffer_size={buffer_size})"
            );
        }
    }

    /// A packet bigger than the configured capacity must not be guillotined
    /// on arrival.
    ///
    /// The trim runs per packet inside the fetch loop, so with a fixed
    /// `buffer_size * 2` capacity an Aaronia streaming 49k-sample packets into
    /// a 16384 capacity lost ~75% of *every* packet before the consumer was
    /// offered any of it. That is not backpressure — the consumer never got
    /// the chance to decline. Live symptom: 61.4 MS/s at the device, 0.6 MS/s
    /// reaching the pipeline, and digital decode unable to hold frame sync.
    #[test]
    fn capacity_floor_accommodates_a_packet_larger_than_the_configured_size() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let mut block = HttpSourceBuilder::new("http://localhost:54664")
            .buffer_size(8192) // what bigear configured: capacity 16384
            .build()
            .expect("Should create HttpSource");

        let configured_capacity = block.buffer_capacity();
        assert_eq!(configured_capacity, 16384, "baseline before any packet");

        // A real packet observed from a Spectran V6 at 61.4 MS/s.
        let packet_samples = 49_152usize;
        assert!(
            packet_samples > configured_capacity,
            "test is meaningless unless the packet exceeds the configured cap"
        );

        block.max_packet_samples = packet_samples;

        assert!(
            block.buffer_capacity() >= packet_samples,
            "one packet must fit whole: capacity {} < packet {}",
            block.buffer_capacity(),
            packet_samples
        );
        assert_eq!(
            block.buffer_capacity(),
            packet_samples * PACKET_CAPACITY_FACTOR,
            "floor should be several packets, for scheduler jitter"
        );
        assert_eq!(
            block.get_stream_stats().buffer_capacity,
            block.buffer_capacity(),
            "reported capacity must still track the enforced one"
        );
    }

    /// The floor only ever raises the capacity; a caller who deliberately
    /// configures a large buffer keeps it.
    #[test]
    fn capacity_floor_never_shrinks_a_generous_configured_buffer() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let mut block = HttpSourceBuilder::new("http://localhost:54664")
            .buffer_size(4_000_000)
            .build()
            .expect("Should create HttpSource");

        block.max_packet_samples = 49_152;
        assert_eq!(
            block.buffer_capacity(),
            8_000_000,
            "configured capacity exceeds the packet floor and must win"
        );
    }

    /// Setting a reference level explicitly must still reach the device.
    ///
    /// The default is `None` so that launching an app does not silently
    /// change the operator's gain — but an explicit request has to work, or
    /// the flag would be inert.
    #[test]
    fn an_explicit_reference_level_is_carried_through() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let built = HttpSourceBuilder::new("http://localhost:54664")
            .reference_level(-18.0)
            .build()
            .expect("Should build");
        assert_eq!(built.reference_level, Some(-18.0));
    }

    #[tokio::test]
    async fn test_url_security_validation() {
        // Test IP address validation warnings
        let source = HttpSourceBuilder::new("http://192.168.1.100:54664");
        assert!(source.build().is_ok()); // Should work but generate warning

        let source = HttpSourceBuilder::new("http://10.0.0.1:54664");
        assert!(source.build().is_ok()); // Should work but generate warning

        // Test localhost variations
        let source = HttpSourceBuilder::new("http://localhost:54664");
        assert!(source.build().is_ok());

        let source = HttpSourceBuilder::new("http://127.0.0.1:54664");
        assert!(source.build().is_ok());

        // Test domain validation
        let source = HttpSourceBuilder::new("http://device.local:54664");
        assert!(source.build().is_ok());
    }

    #[tokio::test]
    async fn test_buffer_configuration() {
        // Test various buffer sizes
        for buffer_size in [1024, 4096, 8192, 16384] {
            let source = HttpSourceBuilder::new("http://localhost:54664").buffer_size(buffer_size);
            assert!(source.build().is_ok());
        }

        // Test with zero buffer size (should still work, though not practical)
        let source = HttpSourceBuilder::new("http://localhost:54664").buffer_size(0);
        assert!(source.build().is_ok());
    }

    #[tokio::test]
    async fn test_timeout_configuration() {
        // Test various timeout values
        for timeout in [1000, 5000, 15000, 30000] {
            let source = HttpSourceBuilder::new("http://localhost:54664").timeout_ms(timeout);
            assert!(source.build().is_ok());
        }

        // Test with very short timeout
        let source = HttpSourceBuilder::new("http://localhost:54664").timeout_ms(100);
        assert!(source.build().is_ok());

        // Test with very long timeout
        let source = HttpSourceBuilder::new("http://localhost:54664").timeout_ms(120000);
        assert!(source.build().is_ok());
    }

    #[tokio::test]
    async fn test_rate_reduction_configuration() {
        // Test various rate reduction factors
        for factor in [2, 4, 8, 10, 16] {
            let source = HttpSourceBuilder::new("http://localhost:54664").rate_reduction(factor);
            assert!(source.build().is_ok());
        }

        // Test without rate reduction
        let source = HttpSourceBuilder::new("http://localhost:54664");
        assert!(source.build().is_ok());
    }

    #[tokio::test]
    async fn test_input_selection() {
        // Test various input names
        for input in ["main", "secondary", "test_input", "{uuid-format}"] {
            let source = HttpSourceBuilder::new("http://localhost:54664").input(input);
            assert!(source.build().is_ok());
        }

        // Test without specific input (auto-select)
        let source = HttpSourceBuilder::new("http://localhost:54664");
        assert!(source.build().is_ok());
    }

    /// The `/stream` query string is assembled in `start_stream` from these
    /// four fields, which needs a live server to exercise end-to-end. What
    /// is checkable here is that every one of them survives the builder —
    /// a dropped field would silently produce a URL missing that parameter.
    #[test]
    fn test_stream_url_construction() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let block = HttpSourceBuilder::new("http://localhost:54664")
            .format(StreamFormat::Float32)
            .input("main")
            .rate_reduction(4)
            .scale(1_000_000.0)
            .build()
            .expect("Should create HttpSource");

        assert_eq!(block.base_url, "http://localhost:54664");
        assert_eq!(block.stream_format, StreamFormat::Float32);
        assert_eq!(block.input_name.as_deref(), Some("main"));
        assert_eq!(block.rate_reduction, Some(4));
        assert_eq!(block.scale, Some(1_000_000.0));
        // `as_str()` is what lands in the query string.
        assert_eq!(block.stream_format.as_str(), "float32");
    }

    /// The builder's `.scale(N)` must reach the block's internal
    /// `StreamParser`, not just the `/stream` query string. The block used
    /// to construct `StreamParser::new(stream_format, None)`, so while the
    /// server was asked to encode with `?scale=N`, decoding fell back to
    /// the full-scale default (32768) and every int16 sample whose packet
    /// carried no per-packet `scale` field came out wrong by a factor of
    /// 32768/N.
    #[tokio::test]
    async fn configured_scale_reaches_the_stream_parser() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let scale = 1000.0_f64;

        // Wire bytes: JSON header (deliberately *without* a per-packet
        // `scale` field, as sent by servers honouring `?scale=N`), a record
        // separator, then one IQ pair as little-endian i16 at ±raw 1000.
        let header = r#"{"startTime":0.0,"endTime":1.0,"startFrequency":100.0,"endFrequency":200.0,"samples":1,"unit":"volt","payload":"iq","minPower":0,"maxPower":1,"sampleSize":2}"#;
        let mut body = header.as_bytes().to_vec();
        body.push(30u8); // ASCII record separator
        for v in [1000i16, -1000] {
            body.extend_from_slice(&v.to_le_bytes());
        }

        let server = MockServer::start().await;
        // Matching on the query param also pins the request side: if
        // `?scale=N` stopped being sent, the mock would 404 and
        // `start_stream` would fail. The other endpoints `start_stream`
        // probes (/info, /config, /control/stream) 404 harmlessly.
        Mock::given(method("GET"))
            .and(path("/stream"))
            .and(query_param("scale", "1000"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;

        let mut source = HttpSourceBuilder::new(&server.uri())
            .format(StreamFormat::Int16)
            .input("main") // skip input auto-discovery
            .scale(scale)
            .build()
            .expect("Should create HttpSource");

        source
            .start_stream()
            .await
            .expect("mock /stream must accept the request");
        let added = source
            .fetch_samples()
            .await
            .expect("the packet should decode");
        assert_eq!(added, 1, "one IQ pair in the payload");

        let s = source.sample_buffer[0];
        assert!(
            (s.re - 1.0).abs() < 1e-6,
            "raw 1000 with ?scale=1000 must decode to 1.0, got {} \
             (0.0305… = 1000/32768 means the configured scale never \
             reached the parser)",
            s.re,
        );
        assert!((s.im - -1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_default_configuration() {
        // Pins the documented defaults on `HttpSourceBuilder::new`
        // (100 MHz / 1 MS/s / Int16 / no auth), so changing one is a
        // deliberate act rather than a silent behaviour change. Previously
        // this only checked that `build()` returned Ok.
        let built = HttpSourceBuilder::new("http://localhost:54664")
            .build()
            .expect("Should build with defaults");

        assert_eq!(built.current_frequency, 100e6);
        assert_eq!(built.current_sample_rate, 1e6);
        // `None`, not a number. A default reference level is pushed to the
        // hardware on the first stream start, so defaulting it to a value
        // meant simply launching an app overwrote the operator's receiver
        // gain with a figure they had never chosen. Absent means "leave the
        // device's gain alone".
        assert_eq!(
            built.reference_level, None,
            "no reference level unless the caller asks for one"
        );
        assert_eq!(built.buffer_size, 4096);
        assert_eq!(built.stream_format, StreamFormat::Int16);
        assert!(matches!(built.auth_method, AuthMethod::None));
        assert_eq!(built.input_name, None, "input is auto-selected by default");
        assert_eq!(built.rate_reduction, None);
        assert_eq!(built.scale, None);
        assert!(!built.stream_active);
    }

    #[tokio::test]
    async fn test_complex_configuration_combinations() {
        // Test complex combinations of settings
        let source = HttpSourceBuilder::new("https://rtsa-device.local:8443")
            .frequency_str("2.4G")
            .expect("Should parse frequency")
            .sample_rate_str("10M")
            .expect("Should parse sample rate")
            .reference_level(-30.0)
            .buffer_size(16384)
            .timeout_ms(45000)
            .format(StreamFormat::Int16)
            .auth(AuthMethod::Token {
                token: "complex-token-abc123".to_string(),
            })
            .input("channel_1")
            .rate_reduction(8);

        assert!(source.build().is_ok());
    }

    #[test]
    fn test_error_handling_invalid_configurations() {
        // Test that invalid frequency strings are handled
        let result =
            HttpSourceBuilder::new("http://localhost:54664").frequency_str("not-a-frequency");
        assert!(result.is_err());

        // Test that invalid sample rate strings are handled
        let result = HttpSourceBuilder::new("http://localhost:54664").sample_rate_str("not-a-rate");
        assert!(result.is_err());

        // Test that malformed URLs are handled
        let source = HttpSourceBuilder::new("malformed-url");
        assert!(source.build().is_err());
    }

    /// A source that has never streamed must report zeroed counters rather
    /// than stale or uninitialised ones — this is the shape an external
    /// dashboard reads before the first packet arrives.
    #[test]
    fn test_stream_stats_structure() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let block = HttpSourceBuilder::new("http://localhost:54664")
            .frequency(146.52e6)
            .sample_rate(2.048e6)
            .format(StreamFormat::Int16)
            .buffer_size(4096)
            .input("test_input")
            .rate_reduction(2)
            .build()
            .expect("Should create HttpSource");

        let stats = block.get_stream_stats();
        assert_eq!(stats.current_frequency, 146.52e6);
        assert_eq!(stats.current_sample_rate, 2.048e6);
        assert_eq!(stats.format, StreamFormat::Int16);
        assert_eq!(stats.input_name.as_deref(), Some("test_input"));
        assert_eq!(stats.dropped_packets, 0, "no packets seen yet");
        assert_eq!(stats.input_msps, 0.0);
        assert_eq!(stats.packet_rate, 0.0);
        assert!(stats.buffer_level <= stats.buffer_capacity);
    }

    #[tokio::test]
    async fn test_https_support() {
        // Test HTTPS URL support
        let source = HttpSourceBuilder::new("https://localhost:54664");
        assert!(source.build().is_ok());

        let source = HttpSourceBuilder::new("https://rtsa-device.local:8443");
        assert!(source.build().is_ok());
    }

    #[tokio::test]
    async fn test_port_configuration() {
        // Test various port configurations
        for port in [54664, 8080, 8443, 9000] {
            let source = HttpSourceBuilder::new(&format!("http://localhost:{}", port));
            assert!(source.build().is_ok());
        }
    }

    #[tokio::test]
    async fn test_edge_case_configurations() {
        // Test minimum valid configuration
        let source = HttpSourceBuilder::new("http://localhost:54664")
            .frequency(1.0) // 1 Hz (extreme low)
            .sample_rate(1.0) // 1 S/s (extreme low)
            .buffer_size(1); // Minimal buffer
        assert!(source.build().is_ok());

        // Test maximum practical configuration
        let source = HttpSourceBuilder::new("http://localhost:54664")
            .frequency(6e9) // 6 GHz (high end)
            .sample_rate(250e6) // 250 MS/s (high end)
            .buffer_size(1_000_000); // Large buffer
        assert!(source.build().is_ok());
    }

    #[test]
    fn test_concurrent_source_creation() {
        // Test that multiple sources can be created concurrently
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let sources: Vec<_> = (0..5)
            .map(|i| {
                HttpSourceBuilder::new(&format!("http://localhost:{}", 54664 + i))
                    .frequency(100e6 + i as f64 * 10e6)
                    .sample_rate(1e6 + i as f64 * 1e6)
                    .build()
            })
            .collect();

        // All sources should be created successfully
        for (i, source) in sources.iter().enumerate() {
            assert!(
                source.is_ok(),
                "Source {} should be created successfully",
                i
            );
        }
    }
}
