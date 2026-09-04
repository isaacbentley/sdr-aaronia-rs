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

/// Move as many samples as fit from the front of `buffer` into `out`,
/// returning how many moved.
///
/// This is `work()`'s output copy, and it runs once per scheduler call at
/// whatever rate the stream sustains — so it is two bulk `copy_from_slice`
/// calls over the deque's contiguous halves rather than a `pop_front` per
/// sample, whose per-element wrap-around check the compiler cannot lift.
/// The samples the transport can deliver (~19 MS/s at 4 bytes each on the
/// measured link) are cheap to move this way and expensive one at a time,
/// and every cycle saved here is one the DSP consumer on the same machine
/// gets back.
fn copy_out(buffer: &mut std::collections::VecDeque<Complex32>, out: &mut [Complex32]) -> usize {
    let n = buffer.len().min(out.len());
    let (front, back) = buffer.as_slices();
    let from_front = front.len().min(n);
    out[..from_front].copy_from_slice(&front[..from_front]);
    let from_back = n - from_front;
    if from_back > 0 {
        out[from_front..n].copy_from_slice(&back[..from_back]);
    }
    // Dropping the drain advances the deque's head; `Complex32` is `Copy`,
    // so no per-element work happens.
    buffer.drain(..n);
    n
}

/// This source's share of the output buffer's size, in samples.
///
/// **It does not gate `work()`.** That is what the name and the original
/// comment here claimed, and it is not what `set_min_items` does: in
/// FutureSDR 0.0.39 the circular buffer reads `min_items` only in
/// `Writer::connect`, where the buffer is sized `writer.min_items +
/// reader.min_items - 1` items rounded up to a page. Nothing consults it
/// afterwards, and the block loop runs `work()` on any buffer notification
/// regardless of how much room there is. What this constant buys is a
/// *buffer* big enough that the writer and the reader each get a useful
/// block without waiting on the other — with the pipeline asking for the
/// same figure, the connection is 131072 samples, so the source can be
/// filling one half while the pipeline drains the other.
///
/// Spinning on a near-empty output is prevented in `work()` instead, by
/// only setting `io.call_again` when there is a reason to run again.
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

/// Longest a `work()` call will park waiting for the next stream chunk.
///
/// Not a rate limit: the wait ends the moment a chunk arrives, which at any
/// real stream rate is well under a millisecond. It bounds how long the
/// block can go without servicing its message inbox, since FutureSDR only
/// reads that between `work()` calls — so it is a shutdown valve, and short
/// enough that Ctrl+C and `--duration-secs` stay prompt.
const CHUNK_WAIT: std::time::Duration = std::time::Duration::from_millis(20);

/// How long the passive link-budget check counts bytes before deciding
/// whether this path can carry the configured span.
///
/// Long enough that chunk quantisation and scheduler jitter are noise —
/// at 61 MB/s this is 120 MB across thousands of chunks — and short
/// enough that the verdict arrives while the operator is still watching
/// the stream start rather than after the capture is written. It runs
/// *after* [`crate::link_budget::LINK_PROBE_SETTLE`], so the answer is
/// available about two and a half seconds in.
const LINK_CHECK_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

/// Fraction of the required byte rate the stream must achieve before the
/// link-budget check calls the path adequate.
///
/// Not slack for a marginal link — slack for the *measurement*. The
/// meter counts decoded IQ payload against the device's reported rate,
/// so a healthy stream sits at 1.0 exactly; what eats into that is sweep
/// quantisation (a packet completing across a sweep boundary credits its
/// samples to the later sweep) and the window's ends landing between
/// sweeps — small over a two-second window of continuous sweeps. Two
/// percent covers them with room to spare, and is below the shortfall
/// that matters: the measured `--span 20M` failure lost ~5%.
const LINK_BUDGET_TOLERANCE: f64 = 0.98;

/// Relative change in the device-reported sample rate that counts as a
/// real retune rather than jitter.
///
/// The device's rate ladder steps in powers of two, so a genuine retune
/// always clears this band by a wide margin, while a rate inferred from
/// `samples / duration` wobbles well inside it. Shared by the
/// link-budget restart in [`HttpSource::note_device_rate`] and the
/// `current_sample_rate` adoption hysteresis, which are the same
/// question: has the device actually moved?
const RATE_CHANGE_BAND: f64 = 0.1;

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
    /// The passive link-budget check's verdict, once it has one for the
    /// current configuration — the predictive complement to
    /// `dropped_packets`, carrying the measured rate, the requirement,
    /// and the widest span that fits. `None` until the check completes,
    /// and reset to `None` when a configuration restart re-arms it.
    pub link_budget: Option<crate::link_budget::LinkBudgetVerdict>,
}

impl Default for StreamStats {
    fn default() -> Self {
        Self {
            active: false,
            format: StreamFormat::CAPTURE_DEFAULT,
            current_frequency: 0.0,
            current_sample_rate: 0.0,
            buffer_level: 0,
            buffer_capacity: 0,
            input_name: None,
            input_msps: 0.0,
            dropped_packets: 0,
            packet_rate: 0.0,
            restart_pending: false,
            link_budget: None,
        }
    }
}

/// The passive link-budget check's lifecycle.
///
/// One field instead of the three hand-coupled ones this replaced (a
/// meter `Option` plus `checked`/`short` bools): four sites had to keep
/// that trio coherent by hand, and combinations like "checked but still
/// armed" were representable. Here they are not — the check is exactly
/// one of unmeasured, measuring, or done, and the verdict data lives in
/// the `Done` state it belongs to.
#[derive(Debug)]
enum LinkCheck {
    /// The current configuration has not been measured; the next stream
    /// start arms a meter (or decides no honest measurement is possible
    /// and goes straight to `Done(None)`).
    Unmeasured,
    /// Counting bytes on the live stream.
    Measuring(crate::link_budget::ThroughputMeter),
    /// Verdict delivered — at most once per configuration. `None` means
    /// the window closed without a usable measurement or comparison
    /// rate: checked, nothing honest to say, and no repeat on
    /// reconnect.
    Done(Option<crate::link_budget::LinkBudgetVerdict>),
}

impl LinkCheck {
    /// A freshly armed meter with this block's settle and window — the
    /// one way a measurement starts, so the two durations cannot drift
    /// apart between the arm sites (stream start, mid-stream retune,
    /// missing-data re-arm).
    fn armed() -> Self {
        LinkCheck::Measuring(crate::link_budget::ThroughputMeter::new(
            crate::link_budget::LINK_PROBE_SETTLE,
            LINK_CHECK_WINDOW,
        ))
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
    /// Items the connected output buffer holds in total, learned from the
    /// writer once the flowgraph has wired it up (0 until then, and for the
    /// unit tests that never connect one). Feeds the `buffer_capacity` floor
    /// so the retention buffer is never smaller than what the consumer is
    /// able to take in a single call.
    downstream_capacity: usize,

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
    /// Raw stream chunks from the background reader task.
    ///
    /// The socket used to be read one chunk per `work()` call, so throughput
    /// was capped by how often the scheduler ran the block — measured at a
    /// tenth of what `curl` pulls from the same endpoint, because between
    /// calls the socket was not read and the server's stream backed up. A
    /// dedicated task now drains the socket continuously into this bounded
    /// channel; `work()` only sweeps already-received chunks and parses them.
    /// The bound provides backpressure: if the consumer falls behind the
    /// channel fills, the reader blocks on `send`, and TCP flow control does
    /// the rest — no unbounded memory.
    /// Total samples dropped by the capacity trim, and the next count that
    /// warrants a log line. See the trim site for why this is rate-limited.
    overflow_samples: u64,
    next_overflow_report: u64,
    /// Total signal time lost to server-side gaps, and the next drop count
    /// that warrants a log line. See the report site: this is loss that never
    /// reached the process, so no sample counter here can show it.
    stream_gap_seconds: f64,
    next_gap_report: u64,
    /// Passive link-budget check, armed at stream start and settled into
    /// its verdict at most once per configuration.
    ///
    /// The complement of `DropDetector`: that one reports gaps the server
    /// left, after the fact. This one counts the IQ payload actually
    /// arriving and compares it against what the configured span *needs*,
    /// so the operator is told the path is too narrow rather than left to
    /// infer it from a corrupted capture. It reuses the stream already
    /// open — no second connection, and no cost beyond adding up sample
    /// counts. A configuration restart resets it to `Unmeasured` so the
    /// new span is measured too; a plain reconnect does not, so a flaky
    /// stream cannot repeat the same warning.
    link_check: LinkCheck,
    /// Latest sample rate the device reported in its packet headers.
    ///
    /// Tracked separately from `current_sample_rate`, which adopts a new
    /// rate only past a 10% hysteresis band to keep the tuning target
    /// stable. The link check must compare against what the device is
    /// *actually* streaming: judged against a requested rate the device
    /// merely came close to, the builder's default 1 MS/s request (served
    /// by the 0.96 MS/s rung, a 4% gap against a 2% tolerance) would warn
    /// on every healthy link.
    link_device_rate: Option<f64>,
    chunk_rx: Option<tokio::sync::mpsc::Receiver<bytes::Bytes>>,
    /// Handle to the reader task, kept so it can be aborted on reconnect or
    /// retune. Dropping the receiver also ends the task (its `send` fails),
    /// but an explicit abort is prompt and unambiguous.
    reader_task: Option<tokio::task::JoinHandle<()>>,

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
        // Security: validate scheme and warn on non-local hosts — the one
        // shared implementation, also used by the link-budget probe.
        crate::http_endpoints::validate_base_url(&base_url)?;

        // Create HTTP endpoints client for advanced control
        let endpoints_client = HttpEndpointsClient::new(base_url.clone(), auth_method.clone())?;

        // Streaming client: the shared RTSA settings (which deliberately
        // set no total timeout — it would cover the whole streamed body),
        // with the caller's connect timeout.
        let streaming_client = crate::http_endpoints::rtsa_client_builder(
            std::time::Duration::from_millis(timeout_ms.min(30000)),
        )
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
            downstream_capacity: 0,
            stream_format,
            stream_parser,
            input_name,
            rate_reduction,
            scale,
            stream_active: false,
            current_frequency: frequency,
            current_sample_rate: sample_rate,
            overflow_samples: 0,
            next_overflow_report: 1,
            stream_gap_seconds: 0.0,
            next_gap_report: 1,
            link_check: LinkCheck::Unmeasured,
            link_device_rate: None,
            chunk_rx: None,
            reader_task: None,
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

        // Build the streaming URL through the shared `StreamParams`
        // serializer — the one `/stream` query-string implementation in
        // the crate, so this URL, the endpoints client's and the
        // link-budget probe's cannot drift apart. Values are
        // percent-encoded: input names come from the server and may
        // contain characters that would corrupt a hand-built query
        // string.
        let stream_url = {
            let mut params =
                crate::http_endpoints::StreamParamsBuilder::new().format(self.stream_format);
            if let Some(ref input) = self.input_name {
                params = params.input(input.clone());
            }
            if let Some(reduction) = self.rate_reduction {
                params = params.rate_reduction(reduction);
            }
            // Server-side scale (?scale=N) for the int16 path, per v9 PDF.
            if let Some(scale) = self.scale {
                params = params.scale(scale);
            }
            params.build().stream_url(&self.base_url)
        };

        info!("Constructed stream URL: {}", stream_url);

        // Apply authentication — the shared definition on `AuthMethod`.
        let request_builder = self
            .auth_method
            .apply_to(self.streaming_client.get(&stream_url));

        info!("Sending stream request to {}.", stream_url);
        info!("Sending HTTP request...");

        let response = request_builder.send().await?;
        info!("Received HTTP response status: {}", response.status());
        // The shared status-to-error mapping, so a failed stream open is
        // the same typed `Error::Http` a failed probe or control call is.
        let response = HttpEndpointsClient::ensure_success("Stream request", response)?;

        // Spawn a task that drains the socket continuously into a bounded
        // channel, decoupling the read rate from how often `work()` runs.
        //
        // 64 chunks (~10 MB at ~157 KiB each) is enough slack that ordinary
        // scheduling jitter never stalls the socket, while still bounding
        // memory and giving real backpressure when the consumer cannot keep
        // up. The task ends when the stream does, or when the receiver is
        // dropped on reconnect/retune.
        let mut stream = response.bytes_stream();
        let (tx, rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(64);
        let task = tokio::spawn(async move {
            use futures::StreamExt;
            while let Some(item) = stream.next().await {
                match item {
                    Ok(chunk) => {
                        // `send` awaits when the channel is full — this is the
                        // backpressure point, and where TCP flow control kicks
                        // in. A send error means the receiver was dropped, so
                        // the source is gone and the task should end.
                        if tx.send(chunk).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("Stream chunk error in reader task: {}", e);
                        break;
                    }
                }
            }
            debug!("Aaronia stream reader task ended");
        });
        self.chunk_rx = Some(rx);
        self.reader_task = Some(task);

        // Arm the passive link-budget check on the stream we have just
        // opened, unless the current configuration already has its
        // verdict (a reconnect loop must not repeat the same warning; a
        // configuration restart resets to `Unmeasured` first). The
        // settle window is anchored here, at the response, so the
        // backlog the server is about to hand over — faster than real
        // time, and the reason a naive measurement reads high — falls
        // inside it.
        if !matches!(self.link_check, LinkCheck::Done(_)) {
            if self.link_check_measurable() {
                self.link_check = LinkCheck::armed();
            } else {
                debug!(
                    "Link-budget check skipped: no honest byte requirement exists for \
                     format {} with rate_reduction {:?}",
                    self.stream_format.as_str(),
                    self.rate_reduction,
                );
                self.link_check = LinkCheck::Done(None);
            }
        }

        self.stream_active = true;
        info!("Advanced Aaronia HTTP streaming initialized");

        Ok(())
    }

    /// Move every queued chunk into `sample_buffer`, returning how many
    /// samples were added.
    ///
    /// `wait` is what an idle caller should do when the channel is empty:
    /// `Some(d)` parks on the channel for up to `d`, `None` returns
    /// immediately. Parking here — rather than on a clock in `work()` — is
    /// the difference between waking on the next chunk (sub-millisecond at
    /// any real rate) and sleeping through 50 ms of stream. See the call
    /// site for the measurement.
    async fn fetch_samples(&mut self, wait: Option<std::time::Duration>) -> Result<usize> {
        {
            debug!("fetch_samples() called");
        }
        trace!("Fetching samples from stream...");

        // Sibling note to `start_stream`: the native-SDK sample-fetch
        // branch was deleted when `NativeSdkClient`'s `get_iq_samples`
        // helper was replaced by the lower-level `to_sample_*` config
        // operations. Use `sdk_source.rs` for native-SDK streaming.

        // Drain every chunk the reader task has queued, parsing each in turn.
        // Sweeping all available chunks (rather than one per call) is what
        // lets a single `work()` move a substantial block, and it keeps the
        // channel from filling and stalling the socket.
        let Some(rx) = self.chunk_rx.as_mut() else {
            warn!("No active stream available for reading");
            return Err(Error::Protocol("No active stream".to_string()));
        };

        // Collect first, then parse, so the `&mut self` borrow for parsing
        // does not overlap the `&mut self.chunk_rx` borrow above.
        let mut chunks: Vec<bytes::Bytes> = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(chunk) => chunks.push(chunk),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    // The reader task ended (stream closed or errored). Report
                    // it so `work()` reconnects, distinct from a healthy read
                    // that simply had nothing ready.
                    if chunks.is_empty() {
                        self.cleanup_stream().await;
                        return Err(Error::StreamClosed("stream reader task ended".to_string()));
                    }
                    break;
                }
            }
        }

        // Nothing was queued and the caller has nothing to flush: park on the
        // channel until the reader task delivers, rather than returning empty
        // and letting the caller sleep on a clock.
        //
        // `recv` is cancel-safe, so losing the race to the timeout cannot
        // consume a chunk. The timeout is not a rate limit — it is a
        // shutdown valve, because the block's message loop only runs
        // between `work()` calls and an unbounded await would defer
        // `Terminate`.
        let mut reader_task_ended = false;
        if chunks.is_empty()
            && let Some(timeout) = wait
        {
            // Scoped so the borrow of `self.chunk_rx` held by the pinned
            // future is released before anything below touches `&mut self`.
            // `Some(v)` is "the channel resolved", `None` is "the timeout won".
            let received: Option<Option<Bytes>> = {
                use futures::future::Either;
                let mut recv = std::pin::pin!(rx.recv());
                match futures::future::select(recv.as_mut(), futures_timer::Delay::new(timeout))
                    .await
                {
                    Either::Left((v, _)) => Some(v),
                    Either::Right(_) => None,
                }
            };
            match received {
                Some(Some(chunk)) => chunks.push(chunk),
                // The reader task ended; same report as `Disconnected`, but
                // flagged rather than handled here so `cleanup_stream` can
                // take `&mut self` once the borrow above is gone.
                Some(None) => reader_task_ended = true,
                None => {}
            }
        }
        if reader_task_ended {
            self.cleanup_stream().await;
            return Err(Error::StreamClosed("stream reader task ended".to_string()));
        }

        // Parse first, then feed the link-budget meter, then report — in
        // that order for two reasons. A parse error must not discard a
        // finished measurement: the error is held until the report has
        // run, or the reconnect would replace a completed meter with a
        // fresh one and the check could stay silent forever on a stream
        // whose parse errors recur faster than a measurement window.
        let mut samples_added = 0usize;
        let mut iq_samples = 0usize;
        let mut parse_error = None;
        for chunk in &chunks {
            trace!("Received HTTP stream chunk: {} bytes", chunk.len());
            match self.process_advanced_stream_data(chunk) {
                Ok((added, iq)) => {
                    samples_added += added;
                    iq_samples += iq;
                }
                Err(e) => {
                    parse_error = Some(e);
                    break;
                }
            }
        }

        // And the meter counts *decoded IQ payload* (IQ samples × bytes
        // a sample), not raw chunk bytes, as one observation per sweep.
        // One observation, because every chunk in a sweep would carry the
        // same drain instant anyway: per-chunk observation let the first
        // post-settle sweep count a whole queued connect backlog at zero
        // elapsed width — exactly the inflation the settle window exists
        // to prevent — and made the meter drop the closing sweep's tail.
        // As one batch, the mark discards the backlog sweep whole.
        // IQ payload rather than wire bytes, because the requirement it
        // is compared against is the IQ rate: raw bytes also count JSON
        // headers and any spectra/histogram packets sharing the stream,
        // which inflate the figure and can wave through a path that is
        // short on the IQ itself. Sweeps that decoded no IQ do not
        // observe at all: a stream that keeps the connection alive
        // without carrying decodable IQ (a paused mission, a mid-stream
        // format change) is a failed measurement, not a 0 MB/s link.
        // (A packet finished in a later sweep credits its samples to
        // that sweep — noise over a two-second window.)
        if iq_samples > 0
            && let LinkCheck::Measuring(meter) = &mut self.link_check
            && let Some(bytes_per_sample) = self.stream_format.iq_bytes_per_sample()
        {
            meter.observe(
                std::time::Instant::now(),
                iq_samples.saturating_mul(bytes_per_sample),
            );
        }
        self.report_link_budget();

        if let Some(e) = parse_error {
            return Err(e);
        }
        trace!("Added {} samples from stream", samples_added);
        Ok(samples_added)
    }

    /// Whether an honest link-budget measurement is possible for the
    /// current configuration.
    ///
    /// Two things rule it out: a format with no fixed bytes per sample
    /// (no byte requirement exists to compare against), and server-side
    /// decimation with any factor other than exactly 1 — the wire then
    /// carries fewer bytes than the device rate implies, so the check
    /// would cry wolf on a healthy link. `Some(0)` is not "no
    /// decimation": it is a value this crate never sends and the device
    /// would refuse, so it reads as unmeasurable rather than as a
    /// pass-through.
    fn link_check_measurable(&self) -> bool {
        self.stream_format.iq_bytes_per_sample().is_some()
            && self.rate_reduction.is_none_or(|n| n == 1)
    }

    /// Report the passive link-budget verdict, once, if it is ready.
    ///
    /// The warning this can raise is the predictive half of the loss
    /// story: `DropDetector` says the server *did* drop data, which the
    /// operator learns only after the capture is already corrupted, while
    /// this says the path cannot carry what was asked for and names the
    /// span that would fit. Every dropped sample is a phase discontinuity
    /// that breaks digital symbol lock, so "narrow the span" is not
    /// tidiness — it is the difference between a decodable capture and an
    /// undecodable one.
    ///
    /// A measurement that did not happen re-arms rather than concluding:
    /// a window that closed without countable IQ, or a rate the device
    /// never reported, says nothing about the path, and a stream that
    /// recovers later still deserves its one verdict. A failed
    /// measurement must never *become* a verdict — reading "0 MB/s" out
    /// of an absent stream would condemn every span on the ladder.
    ///
    /// The verdict is compared against [`Self::link_device_rate`] — the
    /// rate the device reported in its packet headers, not the one the
    /// caller asked for — and published on the shared `StreamStats`, so
    /// programmatic consumers get the figures and not just a log line.
    fn report_link_budget(&mut self) {
        let LinkCheck::Measuring(meter) = &self.link_check else {
            return;
        };
        if !meter.is_complete() {
            return;
        }

        let finished = meter.finish();
        // No usable measurement, or no device-reported rate to judge it
        // against (`current_sample_rate` is seeded from the caller's
        // *request* and must not stand in) — start a fresh window
        // instead of retiring the check: the configuration has not been
        // measured, and the next window may have both halves.
        let (Some(mut measured), Some(rate)) = (finished, self.link_device_rate) else {
            self.link_check = LinkCheck::armed();
            return;
        };
        measured.stream_sample_rate = Some(rate);

        // `judge` owns the arithmetic: requirement, shortness against the
        // tolerance, and a remedy rung halved down from the device's own
        // rate — so it exists on its ladder whatever the receiver clock
        // (the default-clock ladder would name wrong rungs on a full V6).
        let Some(verdict) = crate::link_budget::LinkBudgetVerdict::judge(
            rate,
            self.stream_format,
            measured,
            LINK_BUDGET_TOLERANCE,
        ) else {
            // No byte requirement exists for this format; nothing honest
            // to say now or on any later window.
            self.link_check = LinkCheck::Done(None);
            return;
        };
        let bytes_per_sample = verdict.bytes_per_sample;
        let required = verdict.required_byte_rate;

        // Publish promptly: the per-chunk stats refresh would carry it on
        // the next packet anyway, but a consumer polling right after the
        // log line should see the same verdict the line describes.
        if let Some(ref shared) = self.shared_stats
            && let Ok(mut stats) = shared.write()
        {
            stats.link_budget = Some(verdict.clone());
        }

        // A float32 stream is 8 bytes a sample; where int16 would fit
        // the same rate in half the bytes, say so — switching format is
        // as real a remedy as narrowing the span.
        let format_hint = if self.stream_format == StreamFormat::Float32 {
            " Streaming int16 (4 bytes a sample) would halve the requirement."
        } else {
            ""
        };

        if !verdict.short {
            // Worth one line: an operator who has been bitten by a
            // too-wide span needs to see that this one was checked, not
            // just the absence of a complaint. `info` rather than `debug`
            // because release builds compile `debug!` out.
            info!(
                "Link budget: {:.2} MS/s needs {:.1} MB/s at {bytes_per_sample} bytes a \
                 sample; the stream delivered its IQ payload at {}.",
                rate / 1e6,
                required / 1e6,
                verdict.measured,
            );
            self.link_check = LinkCheck::Done(Some(verdict));
            return;
        }

        let remedy = match (verdict.fit_sample_rate_hz, verdict.fit_span_hz) {
            (Some(fit_rate), Some(fit_span)) => format!(
                "The widest span this path sustains is {:.3} MHz ({:.2} MS/s, {:.1} MB/s) — \
                 a rung of this device's own ladder; narrow the span to that, or put the \
                 RTSA host on a faster link.{format_hint}",
                fit_span / 1e6,
                fit_rate / 1e6,
                fit_rate * bytes_per_sample as f64 / 1e6,
            ),
            _ => {
                // The floor the remedy search bottomed out at — the last
                // rung of the device's own ladder, so the text cites the
                // same figure the search actually rejected.
                let [.., floor] = crate::link_budget::device_ladder(rate);
                format!(
                    "Not even this device's slowest rate ({:.0} kS/s, {:.2} MB/s) fits this \
                     measurement, which points at the link or the server rather than the \
                     span.{format_hint}",
                    floor / 1e3,
                    floor * bytes_per_sample as f64 / 1e6,
                )
            }
        };

        warn!(
            "Link budget: this path cannot carry the configured span. {:.2} MS/s \
             ({:.3} MHz of usable span) needs {:.1} MB/s at {bytes_per_sample} bytes a \
             sample; the stream delivered its IQ payload at {}. The server discards what \
             it cannot send, so the shortfall arrives as gaps in the signal — any \
             \"Stream gap\" warnings, before or after this line, are this same finding \
             measured the other way, and every gap is an unsignalled discontinuity that \
             breaks digital symbol timing. {remedy} The figure covers the whole path — \
             server, network and this host's own ingest — so a consumer that cannot keep \
             up looks the same from here.",
            rate / 1e6,
            crate::usable_bandwidth_hz(rate) / 1e6,
            required / 1e6,
            verdict.measured,
        );
        self.link_check = LinkCheck::Done(Some(verdict));
    }

    /// Track the device-reported sample rate for the link-budget check,
    /// restarting the measurement when the device retunes under it.
    ///
    /// An external retune (RTSA GUI or API — nothing restarts this
    /// stream) changes the rate mid-window: bytes counted at the old
    /// rate must not be judged against the new rate's requirement, in
    /// either direction — and a verdict already delivered was for the
    /// *old* configuration, so a retune after `Done` re-arms too: the
    /// new rate has its own budget and deserves its own one check.
    /// Rung steps are 2x, so a real retune always clears
    /// [`RATE_CHANGE_BAND`]; jitter in a rate derived from
    /// `samples / duration` does not, and must not reset the meter on
    /// every packet.
    fn note_device_rate(&mut self, inferred_rate: f64) {
        if inferred_rate <= 0.0 {
            return;
        }
        if let Some(previous) = self.link_device_rate
            && (inferred_rate - previous).abs() / previous > RATE_CHANGE_BAND
        {
            let rearm = match &self.link_check {
                LinkCheck::Measuring(_) => true,
                LinkCheck::Done(_) => self.link_check_measurable(),
                LinkCheck::Unmeasured => false,
            };
            if rearm {
                debug!(
                    "Device rate changed {:.0} -> {:.0} Hz; restarting the link-budget \
                     check for the new configuration",
                    previous, inferred_rate,
                );
                self.link_check = LinkCheck::armed();
            }
        }
        self.link_device_rate = Some(inferred_rate);
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

        // End the reader task and drop the channel. Aborting is prompt;
        // dropping the receiver would also end it (its next `send` fails) but
        // not until the next chunk arrives.
        if let Some(task) = self.reader_task.take() {
            task.abort();
        }
        self.chunk_rx = None;
        self.stream_active = false;
    }

    /// Parse one stream chunk into the sample buffer.
    ///
    /// Returns `(samples_added, iq_samples)`: everything queued for the
    /// consumer, and the subset decoded from IQ-payload packets. The
    /// split exists for the link-budget meter, which must count only IQ —
    /// spectra/histogram/categories scalars occupy half the wire bytes an
    /// IQ sample does (and decompress to more than arrived), so counting
    /// them as IQ payload inflates the measured rate on mixed missions.
    fn process_advanced_stream_data(&mut self, data: &Bytes) -> Result<(usize, usize)> {
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
        let mut iq_samples_added = 0;

        // Gaps the *server* left in the stream, from the packet timestamps.
        //
        // This result used to be discarded, so the only loss the log ever
        // mentioned was our own capacity trim. That is the less important
        // half: once the trim is sized to the consumer, a gap here is what
        // remains, it is invisible in the sample count (the samples were
        // never sent), and it is exactly the unsignalled discontinuity that
        // breaks a demodulator's symbol timing. It appears whenever the
        // device produces more than the link and the RTSA server can carry —
        // at 4 bytes a sample, 30.7 MS/s is 123 MB/s, past what a gigabit
        // link delivers — and an operator seeing undecodable digital traffic
        // needs to know that is why. Same geometric schedule as the trim,
        // for the same reason.
        for packet in &packets {
            if let crate::http_streaming::DropResult::Drop { gap_seconds } =
                self.drop_detector.observe(packet)
            {
                self.stream_gap_seconds += gap_seconds;
                let drops = self.drop_detector.drops();
                if drops >= self.next_gap_report {
                    // When the link-budget check has already measured this
                    // path short, say so — with the verdict's own figures,
                    // not a pointer at a log line possibly hours up the
                    // scroll: otherwise the operator gets two warnings
                    // that read like two separate problems, when one is
                    // the prediction and this is it coming true. The
                    // verdict is per-configuration (a retune resets it),
                    // so it always describes the span currently streaming.
                    let predicted = match &self.link_check {
                        LinkCheck::Done(Some(v)) if v.short => {
                            let fix = match v.fit_span_hz {
                                Some(span) => {
                                    format!(
                                        " — narrowing to {:.3} MHz of span would fit",
                                        span / 1e6
                                    )
                                }
                                None => String::new(),
                            };
                            format!(
                                " The link-budget check measured this path short at \
                                 connect ({:.1} MB/s delivered against the {:.1} MB/s \
                                 that {:.2} MS/s needs{fix}); this is that shortfall \
                                 arriving as lost signal.",
                                v.measured.byte_rate / 1e6,
                                v.required_byte_rate / 1e6,
                                v.sample_rate / 1e6,
                            )
                        }
                        _ => String::new(),
                    };
                    warn!(
                        "Stream gap: the server has skipped {drops} times \
                         ({:.2} s of signal in total, {:.3} s this time). The \
                         samples were never sent, so this is loss upstream of \
                         this process — the device is producing more than the \
                         link can carry. Digital decoding cannot survive it: \
                         every gap is an unsignalled discontinuity to the \
                         demodulator. Narrow the span or use a faster link.\
                         {predicted}",
                        self.stream_gap_seconds, gap_seconds,
                    );
                    self.next_gap_report = drops.saturating_mul(4);
                }
            }
        }

        for packet in packets {
            // Update current stream metadata from the parsed packet. The
            // packet reports its frequency *range*; the tuned frequency is
            // the center of that range, not its lower edge.
            self.current_frequency = packet.sdr_config.center_frequency;

            // The parser derives the rate from `sampleFrequency` when
            // present and from `samples / duration` otherwise; adopt it
            // when it differs meaningfully from what we believe. IQ
            // packets only, in both trackers: a spectra/histogram header
            // without `sampleFrequency` derives a *frame* rate orders of
            // magnitude below the IQ rate, which would ping-pong the
            // link check into restarting on every interleaved packet and
            // hand it a nonsense yardstick on non-IQ-dominated streams.
            let is_iq = packet.metadata.payload == crate::http_streaming::PayloadType::Iq;
            let inferred_rate = packet.sdr_config.sample_rate;
            if is_iq {
                self.note_device_rate(inferred_rate);
                if inferred_rate > 0.0
                    && (self.current_sample_rate <= 0.0
                        || (inferred_rate - self.current_sample_rate).abs()
                            / self.current_sample_rate
                            > RATE_CHANGE_BAND)
                {
                    debug!(
                        "Sample rate updated from metadata: {:.0} -> {:.0} Hz",
                        self.current_sample_rate, inferred_rate
                    );
                    self.current_sample_rate = inferred_rate;
                }
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
            if is_iq {
                iq_samples_added += packet_samples;
            }
            let capacity = self.buffer_capacity();
            if self.sample_buffer.len() > capacity {
                let overflow = self.sample_buffer.len() - capacity;
                self.sample_buffer.drain(0..overflow);
                // Overflow here means the consumer cannot keep up with the
                // stream — expected and correct at a wide span, where no
                // consumer can process 61 MS/s of a 49 MHz survey in real
                // time, so the buffer keeps only the most recent samples.
                // Counted always; logged on a geometric schedule, because
                // with the continuous reader this fires thousands of times a
                // second at wide span and a line each would bury the log.
                self.overflow_samples = self.overflow_samples.saturating_add(overflow as u64);
                if self.overflow_samples >= self.next_overflow_report {
                    // Says what happened, not why. "The consumer is slower
                    // than the stream" was the only diagnosis offered, and
                    // now that the buffer is sized to the consumer's reach it
                    // is usually the wrong one: what remains is the backlog
                    // the server hands over at connect, which arrives faster
                    // than real time and is stale by definition. Both causes
                    // land here and the trim cannot tell them apart, so name
                    // both rather than assert one.
                    warn!(
                        "Sample buffer overflow: {} samples dropped so far \
                         (capacity {}); the oldest were discarded to keep the \
                         buffer current. Expected in the first moments of a \
                         stream, where the server delivers its backlog faster \
                         than real time; sustained, it means the consumer \
                         cannot keep up",
                        self.overflow_samples, capacity
                    );
                    self.next_overflow_report = self.overflow_samples.saturating_mul(4);
                }
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

        Ok((total_samples_added, iq_samples_added))
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
        // …and never below what the consumer can take in one call, plus a
        // packet of slack. This floor is the one that mattered live: the
        // packet floor came to 32768 samples while the downstream buffer had
        // ~98304 slots free, so the trim discarded 5–9 MS/s that the consumer
        // was ready to accept. Dropping the oldest samples is the right
        // behaviour when nothing downstream *can* take them; it is pure loss
        // when something can.
        let downstream_floor = self
            .downstream_capacity
            .saturating_add(self.max_packet_samples);
        configured.max(packet_floor).max(downstream_floor)
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
            link_budget: match &self.link_check {
                LinkCheck::Done(verdict) => verdict.clone(),
                _ => None,
            },
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
            if let Some(task) = self.reader_task.take() {
                task.abort();
            }
            self.chunk_rx = None;
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
    /// which "always succeeds": it logged a successful tune in a setup where
    /// the device never moved. `/control` answers `success=true` whether or
    /// not any block applies the command — the same device has been measured
    /// both honouring and ignoring the identical full-tuple payload in
    /// different mission states — so the write now goes through
    /// `/remoteconfig` against the block that genuinely carries
    /// `centerfreq0`, and is read back, because that is the only retune whose
    /// success can be proven rather than assumed.
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
                // No block carrying `centerfreq0` (an IQ-demodulator mission
                // exposes `centerfreq`, and other front ends differ), or the
                // config read failed outright. Fall back to the `/control`
                // full-tuple capture command, which is broadcast to whatever
                // block understands it — the pre-0.7.6 behaviour, and the
                // path Aaronia's own demodulator examples use. It cannot be
                // verified (`success=true` regardless), so say so instead of
                // claiming the tune took.
                warn!(
                    "Could not tune via /remoteconfig ({e}); falling back to the \
                     unverified /control capture command"
                );
                let fallback = self
                    .endpoints_client
                    .configure_capture(crate::http_endpoints::CaptureControl {
                        frequency_center: Some(self.current_frequency),
                        frequency_span: Some(self.current_sample_rate),
                        reference_level: self.reference_level.map(|dbm| dbm as f32),
                        control_type: crate::http_endpoints::ControlType::Capture,
                        ..Default::default()
                    })
                    .await;
                match fallback {
                    Ok(_) => info!(
                        "/control capture accepted (unverified: the server answers \
                         success whether or not a block applied it)"
                    ),
                    Err(e) => warn!(
                        "/control capture also failed: {e}. Streaming from whatever \
                         the device is currently tuned to."
                    ),
                }
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
            // A configuration change invalidates the link-budget verdict:
            // the new span has its own requirement, so the check re-arms
            // and measures again — a widened span gets the predictive
            // warning it needs, and a narrowed one stops being blamed by
            // gap warnings for a shortfall measured against the old span.
            // (The shared stats' copy refreshes with the next processed
            // chunk, via `get_stream_stats`.)
            self.link_check = LinkCheck::Unmeasured;
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
        // Learn the connected buffer's size once, for `buffer_capacity`.
        // `max_items` is `usize::MAX` on an unconnected writer, so guard it.
        if self.downstream_capacity == 0 {
            let max = self.output.max_items();
            if max != usize::MAX {
                self.downstream_capacity = max;
            }
        }

        // Keep the buffer stocked rather than topping it up only when it is
        // about to run dry. This was `if self.sample_buffer.len() < o_len`,
        // where `o_len` is the free output space — measured at ~14 samples —
        // so the source refilled only when nearly empty, then dribbled a
        // 40,000-sample chunk out a few samples at a time. Between refills the
        // HTTP socket was not read at all and the server's stream backed up.
        //
        // Each `fetch_samples` sweep pulls every queued chunk, and looping
        // until the buffer is half full keeps the channel drained. The
        // transport ceiling is the link, not this code: measured with
        // `curl` on the same endpoint, ~57 MB/s on the original link and
        // ~75 MB/s (0.6 Gbit/s) station-to-station over WiFi 7 at a
        // 2.4 Gbps PHY — both ends on air halves the medium, and two
        // parallel connections measured *less* in aggregate, so one
        // connection is optimal. At 4 bytes a sample that is ~19 MS/s;
        // 61.44 MS/s (246 MB/s) needs a wired path. The parser itself
        // measures ~3 GB/s (`framing_throughput_meter`), so the decode
        // side never sets the ceiling.
        //
        // The target is bounded by `buffer_capacity()`. It was
        // `(capacity / 2).max(o_len)`, and `o_len` (up to a full downstream
        // buffer) is normally far above the capacity, so the condition could
        // never be satisfied by filling the buffer — every call ran the sweep
        // until the channel ran dry, and the per-packet trim threw away
        // everything past the newest few packets.
        let capacity = self.buffer_capacity();
        let refill_below = (capacity / 2).max(o_len).min(capacity);
        let mut fetches = 0usize;
        while self.sample_buffer.len() < refill_below && fetches < MAX_FETCHES_PER_WORK {
            fetches += 1;
            // Park on the channel only when there is nothing at all to flush.
            // With samples in hand, returning promptly and producing them
            // beats holding them back for a fuller block.
            //
            // This replaced a fixed 50 ms `Delay` at the same point, which
            // was the dominant throughput defect: `work()` drains
            // `sample_buffer` completely, so the next call nearly always
            // found it empty, and any momentary gap in the channel cost
            // 50 ms. Measured live at 15.4 MS/s: ~19 sleeps a second, ~100%
            // of wall clock asleep, 1.24 MS/s reaching the consumer. While
            // asleep the reader task filled the 64-chunk channel and blocked
            // on `send`, so TCP flow control throttled the server and the
            // stream was lost upstream as well.
            let wait = if self.sample_buffer.is_empty() {
                Some(CHUNK_WAIT)
            } else {
                None
            };
            let handle = self.tokio_handle.clone();
            let fetch_res = if let Some(handle) = handle {
                handle.block_on(async { self.fetch_samples(wait).await })
            } else {
                self.fetch_samples(wait).await
            };

            match fetch_res {
                Ok(fetched) => {
                    if fetched == 0 {
                        // Nothing queued, and the wait above (if any) also
                        // came up empty: the stream is idle. Flush whatever
                        // exists and let the next call try again.
                        break;
                    }
                }
                Err(e) => {
                    warn!("Aaronia stream error: {}", e);
                    self.stream_active = false;
                    // Reap the old reader task before reconnecting. On a parse
                    // error the task is still running; `start_stream` would
                    // drop its channel, but a task blocked in `stream.next()`
                    // on a stalled socket only notices at its next send — and
                    // meanwhile it holds a server connection, which on the
                    // free licence (one client) is the very connection the
                    // reconnect needs. `cleanup_stream` aborts it, and is
                    // idempotent when the StreamClosed path already ran it.
                    let handle = self.tokio_handle.clone();
                    if let Some(h) = handle {
                        h.block_on(async { self.cleanup_stream().await });
                    } else {
                        self.cleanup_stream().await;
                    }
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
        let samples_to_copy = copy_out(&mut self.sample_buffer, o);
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

        // Ask to run again only when there is a reason to.
        //
        // This was unconditional, which made the block spin whenever the
        // downstream buffer was momentarily full: `work()` returned having
        // copied nothing and was re-entered immediately, measured at 83,000
        // calls a second burning a core for no samples. Samples still in
        // `sample_buffer` mean the output filled up, and the runtime already
        // wakes this block when the reader consumes, so parking on the inbox
        // is both cheaper and no slower. An empty buffer is the opposite
        // case: come straight back and wait on the socket instead, which
        // `fetch_samples` does with a bounded park rather than a spin.
        io.call_again = self.sample_buffer.is_empty();

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
            buffer_size: 4096, // 4k samples default
            timeout_ms: 15000, // 15s timeout default
            // The one shared definition of the capture default — see
            // `StreamFormat::CAPTURE_DEFAULT` for what else reads it.
            stream_format: StreamFormat::CAPTURE_DEFAULT,
            auth_method: AuthMethod::None, // No auth by default
            input_name: None,              // Auto-select input
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

    /// `copy_out` must drain exactly what it copies, in order, including
    /// when the deque has wrapped and its content spans two slices — the
    /// case a per-sample loop handled implicitly and a bulk copy must
    /// handle explicitly.
    #[test]
    fn copy_out_handles_a_wrapped_deque() {
        let mut dq: std::collections::VecDeque<Complex32> = std::collections::VecDeque::new();
        // Force a wrap: fill, drain some, refill past the old tail.
        for i in 0..8 {
            dq.push_back(Complex32::new(i as f32, 0.0));
        }
        dq.drain(..5);
        for i in 8..14 {
            dq.push_back(Complex32::new(i as f32, 0.0));
        }
        let (a, b) = dq.as_slices();
        assert!(
            !a.is_empty() && !b.is_empty(),
            "test must exercise the two-slice case; got {} / {}",
            a.len(),
            b.len()
        );

        let mut out = [Complex32::new(-1.0, -1.0); 7];
        let n = copy_out(&mut dq, &mut out);
        assert_eq!(n, 7);
        let got: Vec<f32> = out.iter().map(|c| c.re).collect();
        assert_eq!(got, vec![5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0]);
        assert_eq!(dq.len(), 2, "copied samples must be drained");
        assert_eq!(dq.front().unwrap().re, 12.0);

        // Output larger than the deque: takes everything, reports the count.
        let mut big = [Complex32::new(-1.0, -1.0); 8];
        let n = copy_out(&mut dq, &mut big);
        assert_eq!(n, 2);
        assert!(dq.is_empty());
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
        // The reader task delivers chunks asynchronously, so drain in a
        // bounded poll rather than assuming one call synchronously reads the
        // body — which is how the source is actually driven in `work()`.
        let mut added = 0;
        for _ in 0..100 {
            added = source
                .fetch_samples(None)
                .await
                .expect("the packet should decode");
            if added > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
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

    /// The retention buffer must never be smaller than what the consumer can
    /// take in one call.
    ///
    /// This is the half of the throughput defect that lived in
    /// `buffer_capacity`. It read `max(buffer_size * 2, max_packet_samples *
    /// PACKET_CAPACITY_FACTOR)`, which live came to 32768 samples, while the
    /// connected output buffer held 131072 and typically had ~98304 free.
    /// Every `fetch_samples` sweep pushed far more than 32768 samples in and
    /// the per-packet trim dropped the excess — samples the consumer was
    /// ready to accept, discarded inside the source. Measured live at a
    /// 15.4 MS/s stream: 5–9 MS/s trimmed away, 1.24 MS/s reaching the DSP.
    ///
    /// Dropping the oldest samples is right when nothing downstream *can*
    /// take them. It is pure loss when something can, and that is what this
    /// pins.
    #[test]
    fn buffer_capacity_covers_what_the_consumer_can_take() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let mut block = HttpSourceBuilder::new("http://localhost:54664")
            .buffer_size(8192)
            .build()
            .expect("Should create HttpSource");

        // What the flowgraph actually wires up: SOURCE_MIN_OUTPUT_SAMPLES on
        // the writer plus the pipeline's own request on the reader, which
        // FutureSDR sizes as `writer + reader - 1` rounded up to a page.
        block.downstream_capacity = 131_072;
        block.max_packet_samples = 8_192;

        let capacity = block.buffer_capacity();
        assert!(
            capacity >= block.downstream_capacity,
            "capacity {capacity} is below the {} samples the consumer can \
             take in one call — the trim will discard samples that had a \
             home downstream",
            block.downstream_capacity,
        );
        // And the refill target `work()` derives from it must be reachable:
        // `(capacity / 2).max(o_len).min(capacity)` has to be satisfiable by
        // filling the buffer, or the sweep only ever ends when the channel
        // runs dry.
        let o_len = block.downstream_capacity;
        let refill_below = (capacity / 2).max(o_len).min(capacity);
        assert!(
            refill_below <= capacity,
            "refill target {refill_below} exceeds capacity {capacity}; the \
             fetch loop can never satisfy it and will over-fetch every call",
        );
    }

    /// A waiting `fetch_samples` must return when data arrives, not when the
    /// clock runs out.
    ///
    /// The source used to sleep a fixed 50 ms whenever it found the chunk
    /// channel momentarily empty. Because `work()` drains `sample_buffer`
    /// completely, the next call nearly always found it empty, so live this
    /// fired ~19 times a second — ~100% of wall clock asleep — and the
    /// stream was sampled in 50 ms strides. Parking on the channel instead
    /// wakes on the next chunk.
    #[tokio::test]
    async fn a_waiting_fetch_wakes_on_the_chunk_not_the_timeout() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let header = r#"{"startTime":0.0,"endTime":1.0,"startFrequency":100.0,"endFrequency":200.0,"samples":1,"unit":"volt","payload":"iq","minPower":0,"maxPower":1,"sampleSize":2}"#;
        let mut body = header.as_bytes().to_vec();
        body.push(30u8);
        for v in [1000i16, -1000] {
            body.extend_from_slice(&v.to_le_bytes());
        }

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/stream"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;

        let mut source = HttpSourceBuilder::new(&server.uri())
            .format(StreamFormat::Int16)
            .input("main")
            .build()
            .expect("Should create HttpSource");
        source
            .start_stream()
            .await
            .expect("mock /stream must accept the request");

        // A generous timeout, so "returned quickly" cannot be the timeout
        // firing. The chunk is in flight from the reader task.
        let timeout = std::time::Duration::from_secs(5);
        let start = std::time::Instant::now();
        let added = source
            .fetch_samples(Some(timeout))
            .await
            .expect("the packet should decode");
        let elapsed = start.elapsed();

        assert_eq!(added, 1, "one IQ pair in the payload");
        assert!(
            elapsed < timeout / 2,
            "waited {elapsed:?} for a chunk that was already on its way; a \
             wait that tracks the clock rather than the data is the 50 ms \
             sleep returning",
        );
    }

    /// Build a source with its link meter armed at `t0`, as
    /// `start_stream` would, and feed it a steady IQ-payload byte rate
    /// until the measurement window closes.
    ///
    /// `device_rate_hz` is what the packet headers reported (the check
    /// compares against the device's rate, never the caller's request);
    /// pass 0.0 for a stream that never reported one.
    ///
    /// Synthetic instants rather than sleeps: the settle window is half a
    /// second and the counting window two, so a real-time version of this
    /// would be a two-and-a-half-second test with a flaky edge.
    fn source_with_link_measurement(
        device_rate_hz: f64,
        bytes_per_10ms: usize,
    ) -> (HttpSource, std::time::Instant) {
        let mut block = HttpSourceBuilder::new("http://localhost:54664")
            .format(StreamFormat::Int16)
            .build()
            .expect("Should create HttpSource");
        block.link_device_rate = (device_rate_hz > 0.0).then_some(device_rate_hz);

        let t0 = std::time::Instant::now();
        block.link_check = LinkCheck::Measuring(crate::link_budget::ThroughputMeter::starting_at(
            t0,
            crate::link_budget::LINK_PROBE_SETTLE,
            LINK_CHECK_WINDOW,
        ));
        for tick in 0..1_000u64 {
            let at = t0 + std::time::Duration::from_millis(tick * 10);
            let LinkCheck::Measuring(meter) = &mut block.link_check else {
                panic!("the check must stay armed while the trace runs");
            };
            if meter.observe(at, bytes_per_10ms) {
                break;
            }
        }
        (block, t0)
    }

    /// A path that cannot carry the configured span says so — before the
    /// capture is written, not after.
    ///
    /// This is the whole point of the check. `--span 20M` streams at
    /// 30.72 MS/s, which at 4 bytes a sample needs 123 MB/s; a gigabit
    /// path measured ~5% short and lost 1.84 s of a 35 s capture, and
    /// every one of those gaps is a phase discontinuity that breaks
    /// digital symbol lock. Here the stream needs 61.44 MB/s (15.36 MS/s)
    /// and the path delivers ~55.
    #[test]
    fn a_path_short_of_the_configured_span_is_reported_once() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        // 550 kB per 10 ms == 55 MB/s, against the 61.44 MB/s that
        // 15.36 MS/s needs: a 10% shortfall.
        let (mut block, _t0) = source_with_link_measurement(15_360_000.0, 550_000);
        block.report_link_budget();

        let LinkCheck::Done(Some(verdict)) = &block.link_check else {
            panic!("55 MB/s cannot carry a stream that needs 61.4; the operator must be told");
        };
        assert!(verdict.short);
        // The remedy is halved down from the device's own rate: 55 MB/s
        // carries the 7.68 MS/s rung, 6.144 MHz of span.
        assert_eq!(verdict.fit_sample_rate_hz, Some(7_680_000.0));
        assert_eq!(verdict.fit_span_hz, Some(6_144_000.0));
        assert_eq!(verdict.sample_rate, 15_360_000.0);
        assert_eq!(verdict.required_byte_rate, 61_440_000.0);

        // Idempotent: a second call cannot produce a second warning, and
        // the verdict stands.
        block.report_link_budget();
        assert!(matches!(&block.link_check, LinkCheck::Done(Some(v)) if v.short));
    }

    /// The span the operator would be given is one that was measured
    /// working: 55 MB/s carries the 7.68 MS/s rung, so the advice is
    /// 6.144 MHz of span rather than a vague "use a smaller span" — and
    /// it is reached by halving the device's own rate, so it exists on
    /// the device's ladder whatever its receiver clock.
    #[test]
    fn the_remedy_names_a_span_that_actually_fits() {
        let fit_rate = crate::link_budget::max_sustainable_sample_rate_below(
            15_360_000.0,
            55e6,
            StreamFormat::Int16,
        )
        .expect("55 MB/s fits a rung below 15.36 MS/s");
        assert_eq!(fit_rate, 7_680_000.0);
        assert_eq!(crate::usable_bandwidth_hz(fit_rate), 6_144_000.0);
        assert!(
            crate::link_budget::required_byte_rate_for_format(fit_rate, StreamFormat::Int16)
                .expect("int16 has a budget")
                <= 55e6,
            "the span offered as the remedy must itself fit the measurement",
        );
    }

    /// A path that keeps up says nothing alarming, even though the
    /// arithmetic lands within a couple of percent of the requirement.
    ///
    /// The margin here is not slack for a marginal link: the measurement
    /// itself is quantised by sweep boundaries. If `LINK_BUDGET_TOLERANCE`
    /// were 1.0, a perfectly healthy stream would be condemned by
    /// rounding.
    #[test]
    fn a_path_that_keeps_up_raises_no_warning() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        // Exactly the required rate: 61.44 MB/s for 15.36 MS/s.
        let (mut block, _t0) = source_with_link_measurement(15_360_000.0, 614_400);
        block.report_link_budget();

        let LinkCheck::Done(Some(verdict)) = &block.link_check else {
            panic!("a completed measurement with a known rate must produce a verdict");
        };
        assert!(
            !verdict.short,
            "a stream delivered at exactly the rate it needs is not a link problem",
        );
        assert_eq!(verdict.fit_span_hz, None, "no remedy when nothing is wrong");
    }

    /// The stale-rate regression: the builder's default 1 MS/s request is
    /// served by the 0.96 MS/s rung, and the 10% adoption hysteresis
    /// keeps `current_sample_rate` at the request. The check must judge
    /// against the *device's* 0.96 MS/s — judged against the request, a
    /// perfectly healthy default-configuration stream (3.84 MB/s
    /// delivered vs the 3.92 MB/s threshold a 1 MS/s requirement sets)
    /// would warn on every start.
    #[test]
    fn a_request_a_few_percent_above_the_rung_does_not_false_alarm() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        // Device on the 0.96 MS/s rung, delivering exactly its payload
        // rate: 38.4 kB per 10 ms == 3.84 MB/s.
        let (mut block, _t0) = source_with_link_measurement(960_000.0, 38_400);
        assert_eq!(
            block.current_sample_rate, 1e6,
            "the hysteresis keeps the requested rate; the check must not use it",
        );
        block.report_link_budget();

        let LinkCheck::Done(Some(verdict)) = &block.link_check else {
            panic!("the healthy default configuration must still be checked");
        };
        assert!(
            !verdict.short,
            "a device streaming its rung at full rate is not a link problem \
             (measured {} B/s, required {} B/s)",
            verdict.measured.byte_rate, verdict.required_byte_rate,
        );
        assert_eq!(verdict.sample_rate, 960_000.0);
    }

    /// An external retune (RTSA GUI/API — nothing restarts this stream)
    /// mid-measurement restarts the meter: bytes counted at the old rate
    /// must not be judged against the new rate's requirement. Jitter
    /// below the 10% band must not restart it.
    #[test]
    fn a_device_retune_mid_measurement_restarts_the_check() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let (mut block, _t0) = source_with_link_measurement(15_360_000.0, 614_400);
        assert!(matches!(&block.link_check, LinkCheck::Measuring(m) if m.is_complete()));

        // A real retune is a rung step, at least 2x.
        block.note_device_rate(30_720_000.0);
        assert!(
            matches!(&block.link_check, LinkCheck::Measuring(m) if !m.is_complete()),
            "a mixed-rate count must be thrown away, not reported",
        );
        assert_eq!(block.link_device_rate, Some(30_720_000.0));
        block.report_link_budget();
        assert!(
            matches!(block.link_check, LinkCheck::Measuring(_)),
            "the fresh meter has nothing to report yet",
        );

        // Rate jitter (a rate derived from samples/duration wobbles by
        // far less than a rung) leaves the measurement running.
        let (mut block, _t0) = source_with_link_measurement(15_360_000.0, 614_400);
        block.note_device_rate(15_400_000.0);
        assert!(matches!(&block.link_check, LinkCheck::Measuring(m) if m.is_complete()));

        // A retune *after* the verdict re-arms too: the verdict described
        // the old configuration's budget, and the new rate deserves its
        // own single check rather than streaming unjudged behind a stale
        // "checked" flag.
        let (mut block, _t0) = source_with_link_measurement(15_360_000.0, 614_400);
        block.report_link_budget();
        assert!(matches!(&block.link_check, LinkCheck::Done(Some(_))));
        block.note_device_rate(30_720_000.0);
        assert!(
            matches!(&block.link_check, LinkCheck::Measuring(m) if !m.is_complete()),
            "a retune past Done must start a fresh measurement for the new rate",
        );
    }

    /// No measurement, no verdict.
    ///
    /// A stream that never delivered enough to close the window, and a
    /// stream whose rate the device never reported, are both failures of
    /// the *measurement*. Turning either into "0 MB/s" would condemn
    /// every span on the ladder on no evidence, which is exactly the
    /// failure mode this check exists to avoid.
    #[test]
    fn a_failed_measurement_never_warns() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        // Window never closes: a couple of chunks and then silence.
        let mut block = HttpSourceBuilder::new("http://localhost:54664")
            .build()
            .expect("Should create HttpSource");
        block.link_device_rate = Some(15_360_000.0);
        let t0 = std::time::Instant::now();
        block.link_check = LinkCheck::Measuring(crate::link_budget::ThroughputMeter::starting_at(
            t0,
            crate::link_budget::LINK_PROBE_SETTLE,
            LINK_CHECK_WINDOW,
        ));
        for tick in 0..80u64 {
            let LinkCheck::Measuring(meter) = &mut block.link_check else {
                panic!("armed");
            };
            meter.observe(t0 + std::time::Duration::from_millis(tick * 10), 1_000);
        }
        block.report_link_budget();
        assert!(
            matches!(block.link_check, LinkCheck::Measuring(_)),
            "an unfinished measurement stays armed rather than reporting",
        );

        // Window closes, but the device never reported a sample rate, so
        // there is nothing to compare the bytes against: re-arm and wait
        // for a window where both halves exist, rather than retiring the
        // check on a failed measurement — the configuration was never
        // actually judged, and a stream that starts reporting its rate a
        // moment later still deserves its one verdict.
        let (mut unknown_rate, _t0) = source_with_link_measurement(0.0, 1_000);
        unknown_rate.report_link_budget();
        assert!(
            matches!(&unknown_rate.link_check, LinkCheck::Measuring(m) if !m.is_complete()),
            "no known stream rate means no requirement — re-arm, don't conclude",
        );
    }

    /// The verdict is data on the shared stats surface, not only a log
    /// line — and a configuration restart clears it along with the rest
    /// of the check's state, so consumers never read a verdict about a
    /// span that is no longer configured.
    #[test]
    fn the_verdict_reaches_stream_stats_and_a_restart_clears_it() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let (mut block, _t0) = source_with_link_measurement(15_360_000.0, 550_000);
        block.report_link_budget();

        let verdict = block
            .get_stream_stats()
            .link_budget
            .expect("the verdict must be published on StreamStats");
        assert!(verdict.short);
        assert_eq!(verdict.fit_span_hz, Some(6_144_000.0));

        // What the restart_pending path does before reconnecting.
        block.link_check = LinkCheck::Unmeasured;
        assert!(
            block.get_stream_stats().link_budget.is_none(),
            "a stale verdict must not describe a configuration that no longer exists",
        );
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
