//! The `/stream` client both HTTP front-ends run on.
//!
//! `HttpSource`, the FutureSDR block, and `SpectranSource`'s HTTP backend
//! read the same transport, and everything between the socket and an IQ
//! packet is done here, once:
//!
//! - **Opening `/stream`** through [`HttpEndpointsClient::open_stream`], whose
//!   response headers must arrive within [`STREAM_STALL_TIMEOUT`].
//! - **Reading it** on a task that drains the body into a bounded channel and
//!   gives the connection up when it delivers no bytes for
//!   [`STREAM_STALL_TIMEOUT`] — a hung server or a half-open TCP connection,
//!   not a slow link.
//! - **Decoding chunks** with [`StreamParser::process_data_recovering`], so an
//!   undecodable packet costs itself and becomes a gap where it sat, and
//!   running a [`DropDetector`] over the IQ packets to find what the server
//!   skipped.
//! - **Reconnecting** with a front-end's [`ReconnectPolicy`] (backoff, attempt
//!   budget, the uptime that earns the budget back), re-applying tuning and
//!   bracketing connections with `/control` streaming switches as the front-end
//!   asks — and writing none of it while the shared `listen_only` flag is set.
//! - **Restarting** on request, for a front-end that retunes by reconnecting.
//! - **Reporting breaks** in stream order, as [`StreamEvent::Break`] items
//!   between the packets they separate.
//!
//! What a front-end still owns is what it does with the packets: `HttpSource`
//! buffers them for its output port and keys breaks on output sample indices;
//! `SpectranSource` labels them with their capture geometry for reads that never
//! span a gap, retune or rate change.
//!
//! The engine is driven by its owner, not by a task of its own: a front-end
//! asks for chunks with [`HttpStreamEngine::next_chunks`] — which is also where
//! a lost connection is noticed and the next one opened — and decodes them with
//! [`HttpStreamEngine::decode`]. `HttpSource` calls both from `work()`;
//! `SpectranSource` from the task that feeds its reads.

use crate::http_endpoints::{
    CaptureControl, ControlType, HttpEndpointsClient, STREAM_STALL_TIMEOUT, StreamParams,
};
use crate::http_streaming::{
    DropDetector, DropResult, ParsedItem, PayloadType, StreamPacket, StreamParser,
};
use crate::{Error, Result};
use bytes::Bytes;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::time::Instant;
use tracing::{debug, error, info, warn};

/// Depth of the channel between a connection's reader task and the engine,
/// in chunks, and the most chunks one [`HttpStreamEngine::next_chunks`]
/// returns.
///
/// Enough slack that ordinary scheduling jitter never stalls the socket,
/// while still bounding memory and giving real backpressure when the consumer
/// cannot keep up: the reader blocks on `send` and TCP flow control does the
/// rest. Measured against a live server, hyper's adaptive read settles at
/// 64 KiB a chunk (71% of them; 32 KiB for most of the rest), so the queue
/// holds roughly 4 MB — about 45 ms at the 88 MB/s a WiFi 6E path delivers.
pub(crate) const CHUNK_CHANNEL_DEPTH: usize = 64;

/// How a front-end reconnects after its stream is lost.
///
/// Every failure — a connection that ended, stalled or lost its framing, or
/// an open that did not succeed — spends one attempt and waits the current
/// delay, which starts at `first_delay` and doubles up to `max_delay`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ReconnectPolicy {
    /// Delay before the first reconnect attempt after a healthy stretch.
    pub(crate) first_delay: Duration,
    /// Ceiling for the doubling delay.
    pub(crate) max_delay: Duration,
    /// Attempts allowed before the engine gives up, or `None` to keep
    /// trying for as long as it is driven. `Some(0)` never reconnects.
    pub(crate) attempts: Option<u32>,
    /// How long a connection must survive before it counts as healthy and
    /// earns back the full budget. Gating this on uptime rather than on "a
    /// packet arrived" matters: a server that accepts, sends one packet and
    /// hangs up would otherwise reset the budget every round and be
    /// reconnected forever.
    pub(crate) healthy_after: Duration,
}

/// The reconnect budget a [`ReconnectPolicy`] allows, as it is spent.
#[derive(Debug)]
struct Backoff {
    policy: ReconnectPolicy,
    spent: u32,
    delay: Duration,
}

impl Backoff {
    fn new(policy: ReconnectPolicy) -> Self {
        Self {
            policy,
            spent: 0,
            delay: policy.first_delay,
        }
    }

    /// Spend one attempt and return the delay before it, or `None` once the
    /// budget is gone.
    fn next(&mut self) -> Option<Duration> {
        if let Some(allowed) = self.policy.attempts
            && self.spent >= allowed
        {
            if allowed > 0 {
                error!("HTTP stream did not recover after {allowed} reconnect attempts; giving up");
            }
            return None;
        }
        self.spent += 1;
        let delay = self.delay;
        self.delay = self.delay.saturating_mul(2).min(self.policy.max_delay);
        Some(delay)
    }

    /// Restore the full budget and the first delay.
    fn reset(&mut self) {
        self.spent = 0;
        self.delay = self.policy.first_delay;
    }
}

/// The capture tuning re-applied before each reconnect attempt.
///
/// Tuning lives on the device, not the stream, so a plain reconnect usually
/// keeps it — but an RTSA-Suite that restarted comes back on its mission's
/// frequency, and the stream would silently deliver the wrong band. The owner
/// shares this with whatever retunes it, so a reconnect restores the *current*
/// tuning rather than whatever was configured at startup.
// Only `SpectranSource` constructs one; a FutureSDR-only build has no use
// for it beyond the engine's `None`.
#[cfg_attr(not(feature = "file"), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
pub(crate) struct CaptureTuning {
    pub(crate) center_frequency_hz: f64,
    pub(crate) sample_rate_hz: f64,
    pub(crate) reference_level_dbm: f64,
}

impl CaptureTuning {
    /// This tuning as a `/control` capture command. The full tuple, always:
    /// the server answers `success` to a capture `PUT` carrying only one of
    /// the two frequency fields and ignores it.
    pub(crate) fn control(&self) -> CaptureControl {
        CaptureControl {
            frequency_center_hz: Some(self.center_frequency_hz),
            frequency_span_hz: Some(self.sample_rate_hz),
            reference_level_dbm: Some(self.reference_level_dbm as f32),
            control_type: ControlType::Capture,
            ..Default::default()
        }
    }
}

/// What a front-end tells the engine when it builds one.
pub(crate) struct EngineOptions {
    /// Opens `/stream`. A separate client from `control` only so a front-end
    /// can give the stream its own connect timeout.
    pub(crate) stream_client: HttpEndpointsClient,
    /// Makes the control-plane writes around each connection.
    pub(crate) control_client: HttpEndpointsClient,
    /// What to ask `/stream` for.
    pub(crate) params: StreamParams,
    /// While set, the engine writes nothing to the device: no re-tune, no
    /// `/control` streaming switch. Read at each write, so a front-end that
    /// shares it can turn it on mid-stream.
    pub(crate) listen_only: Arc<AtomicBool>,
    pub(crate) reconnect: ReconnectPolicy,
    /// Tuning to re-apply, as a full `/control` capture tuple, before every
    /// reconnect attempt; `None` to leave the device's tuning alone.
    pub(crate) retune_on_reconnect: Option<Arc<Mutex<CaptureTuning>>>,
    /// Write `/control` streaming start before each open and stop after each
    /// connection ends.
    pub(crate) streaming_control: bool,
}

/// What [`HttpStreamEngine::next_chunks`] produced.
#[derive(Debug)]
pub(crate) enum Chunks {
    /// Raw body chunks, oldest first, at most [`CHUNK_CHANNEL_DEPTH`].
    Got(Vec<Bytes>),
    /// Nothing within the wait: the stream is quiet, or between connections.
    Idle,
    /// The reconnect budget is spent; nothing more will arrive.
    Finished,
}

/// One item of a decoded chunk, in stream order.
// Nearly every item is an `Iq`, so boxing it to shrink the rest would add an
// allocation per packet on the hot path for nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub(crate) enum StreamEvent {
    /// An IQ packet.
    Iq(StreamPacket),
    /// The stream is not continuous between the packet before this and the
    /// packet after it.
    Break(StreamGap),
    /// A spectra, histogram or category packet, skipped. `undecoded` is why
    /// the parser could not decode it, when it could not — a compressed
    /// spectra packet carries no byte length to frame it by. Either way its
    /// loss costs no IQ, so it is not a break.
    NonIq {
        payload: PayloadType,
        undecoded: Option<Error>,
    },
}

/// Why the stream broke.
// The details are what the FutureSDR block reports; `SpectranSource` needs
// only that a break happened, and the drop total.
#[cfg_attr(not(feature = "futuresdr"), allow(dead_code))]
#[derive(Debug)]
pub(crate) enum StreamGap {
    /// The connection was replaced — it ended, stalled, lost its framing or
    /// was restarted — and this is the first IQ packet of a new one. The
    /// server restarts its stream at the present moment, so whatever passed
    /// in between is gone.
    Reconnect,
    /// The server skipped signal: the packet timestamps jumped by more than
    /// the [`DropDetector`]'s tolerance. `drops` is the detector's running
    /// total, this one included.
    Server { gap_seconds: f64, drops: u64 },
    /// The timestamps stepped backwards. Nothing is missing, but the packets
    /// do not follow one another either. `steps` is the running total.
    Backward { overlap_seconds: f64, steps: u64 },
    /// An IQ packet, or one whose header could not be read, framed but could
    /// not be decoded. Its samples are gone. The drop detector has been
    /// re-seeded, so the hole is not counted again as a server gap.
    Lost(Error),
}

/// Parser and drop detector for the packets of one stream, across its
/// connections.
struct StreamDecoder {
    parser: StreamParser,
    drops: DropDetector,
    /// A connection was replaced since the last IQ packet.
    reconnected: bool,
}

impl StreamDecoder {
    fn new(params: &StreamParams) -> Result<Self> {
        // The parser must know the server-side `?scale=N` the stream is
        // opened with, or int16 decoding falls back to the full-scale default
        // and every sample is off by a factor of 32768/N.
        Ok(Self {
            parser: StreamParser::new(params.format, params.scale)?,
            drops: DropDetector::default(),
            reconnected: false,
        })
    }

    fn decode(&mut self, chunk: &Bytes) -> Result<Vec<StreamEvent>> {
        // Only build the (allocating) hex/ASCII preview when DEBUG logging is
        // actually enabled — this runs on every chunk of the hot path.
        if !chunk.is_empty() && tracing::enabled!(tracing::Level::DEBUG) {
            let preview = &chunk[..chunk.len().min(100)];
            let ascii: String = preview
                .iter()
                .take(50)
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
                chunk.len(),
                &preview[..preview.len().min(20)],
                ascii
            );
        }

        let items = self.parser.process_data_recovering(chunk)?;
        let mut events = Vec::with_capacity(items.len() + 1);
        for item in items {
            let packet = match item {
                ParsedItem::Packet(packet) => packet,
                ParsedItem::Lost(e) => {
                    // The next packet's timestamp will show the hole, and it
                    // is ours, not the server's. The jitter calibration
                    // survives: losing a packet does not change how the server
                    // stamps the ones after it.
                    self.drops.resync();
                    events.push(StreamEvent::Break(StreamGap::Lost(e)));
                    continue;
                }
                ParsedItem::Skipped { payload, reason } => {
                    events.push(StreamEvent::NonIq {
                        payload,
                        undecoded: Some(reason),
                    });
                    continue;
                }
            };
            // IQ only from here on, through the drop detector too: a spectra
            // frame stamped between two IQ packets would read as a jump in
            // the IQ timeline.
            if packet.metadata.payload != PayloadType::Iq {
                events.push(StreamEvent::NonIq {
                    payload: packet.metadata.payload.clone(),
                    undecoded: None,
                });
                continue;
            }
            if std::mem::take(&mut self.reconnected) {
                events.push(StreamEvent::Break(StreamGap::Reconnect));
            }
            match self.drops.observe(&packet) {
                DropResult::Continuous => {}
                DropResult::Drop { gap_seconds } => {
                    events.push(StreamEvent::Break(StreamGap::Server {
                        gap_seconds,
                        drops: self.drops.drops(),
                    }));
                }
                DropResult::Backward { overlap_seconds } => {
                    events.push(StreamEvent::Break(StreamGap::Backward {
                        overlap_seconds,
                        steps: self.drops.backward_steps(),
                    }));
                }
            }
            events.push(StreamEvent::Iq(packet));
        }
        Ok(events)
    }

    /// The connection was replaced: drop any partial packet from the old one,
    /// or its header would be applied to the new stream's first bytes, and
    /// let the drop detector re-seed its timestamp history rather than count
    /// the outage as one enormous server gap. `resync`, not `reset`: the
    /// cumulative counters are read as monotonic totals. The next IQ packet
    /// is preceded by [`StreamGap::Reconnect`].
    fn connection_replaced(&mut self) {
        self.parser.reset();
        self.drops.resync();
        self.reconnected = true;
    }
}

/// A live `/stream` connection.
struct Connection {
    rx: tokio::sync::mpsc::Receiver<Bytes>,
    /// `None` only for a test that feeds `rx` itself.
    reader: Option<tokio::task::JoinHandle<()>>,
    opened: Instant,
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Prompt and unambiguous: dropping the receiver would also end the
        // reader, but only at its next `send` — and meanwhile it holds a
        // server connection, which on the free licence (one client) is the
        // very connection the reconnect needs.
        if let Some(reader) = self.reader.take() {
            reader.abort();
        }
    }
}

enum Link {
    /// No connection; the next attempt is due at `due`.
    Down {
        due: Instant,
    },
    Up(Connection),
    /// [`HttpStreamEngine::decode`] lost the framing on this connection;
    /// [`HttpStreamEngine::next_chunks`] ends it, which takes an `await`
    /// `decode` cannot make.
    Broken(Connection),
    /// The reconnect budget is spent.
    Finished,
}

/// One HTTP IQ stream, across however many connections it takes. See the
/// module documentation.
pub(crate) struct HttpStreamEngine {
    stream_client: HttpEndpointsClient,
    control_client: HttpEndpointsClient,
    params: StreamParams,
    listen_only: Arc<AtomicBool>,
    retune_on_reconnect: Option<Arc<Mutex<CaptureTuning>>>,
    streaming_control: bool,
    decoder: StreamDecoder,
    backoff: Backoff,
    link: Link,
    /// Whether a connection has been attempted: every later attempt is a
    /// reconnect.
    attempted: bool,
    /// Connections opened so far.
    connections: u64,
}

impl HttpStreamEngine {
    /// An engine with no connection yet; the first [`Self::next_chunks`]
    /// opens one.
    pub(crate) fn new(options: EngineOptions) -> Result<Self> {
        options.params.validate()?;
        Ok(Self {
            decoder: StreamDecoder::new(&options.params)?,
            stream_client: options.stream_client,
            control_client: options.control_client,
            params: options.params,
            listen_only: options.listen_only,
            retune_on_reconnect: options.retune_on_reconnect,
            streaming_control: options.streaming_control,
            backoff: Backoff::new(options.reconnect),
            link: Link::Down {
                due: Instant::now(),
            },
            attempted: false,
            connections: 0,
        })
    }

    fn listen_only(&self) -> bool {
        self.listen_only.load(Ordering::Relaxed)
    }

    /// Up to [`CHUNK_CHANNEL_DEPTH`] raw body chunks, opening or reopening
    /// the connection when one is due.
    ///
    /// The bound on one call is what lets a caller size its buffers for a
    /// sweep: chunks queued past it wait for the next call. `wait` applies
    /// only when nothing is queued — `Some(d)` waits at most `d` for a chunk
    /// or a due reconnect (`Duration::ZERO` returns at once, with no timer),
    /// `None` until there is a chunk or the engine gives up. Parking on the
    /// channel rather than on a clock wakes on the next chunk, which at any
    /// real rate is well under a millisecond. A connection attempt, when one
    /// is due, runs to completion whatever the wait.
    pub(crate) async fn next_chunks(&mut self, wait: Option<Duration>) -> Chunks {
        let deadline = wait.map(|d| Instant::now().checked_add(d).unwrap_or_else(Instant::now));
        loop {
            match &mut self.link {
                Link::Finished => return Chunks::Finished,
                Link::Broken(_) => self.end_connection().await,
                Link::Down { due } => {
                    let due = *due;
                    if due > Instant::now() {
                        match deadline {
                            Some(limit) if limit < due => {
                                if limit > Instant::now() {
                                    tokio::time::sleep_until(limit).await;
                                }
                                return Chunks::Idle;
                            }
                            _ => tokio::time::sleep_until(due).await,
                        }
                    }
                    let opened = self.attempt().await.is_ok();
                    if !opened && deadline.is_some_and(|limit| Instant::now() >= limit) {
                        return Chunks::Idle;
                    }
                }
                Link::Up(connection) => {
                    let mut chunks = Vec::new();
                    let mut ended = false;
                    while chunks.len() < CHUNK_CHANNEL_DEPTH {
                        match connection.rx.try_recv() {
                            Ok(chunk) => chunks.push(chunk),
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => {
                                ended = true;
                                break;
                            }
                        }
                    }
                    if !chunks.is_empty() {
                        return Chunks::Got(chunks);
                    }
                    if !ended {
                        // `recv` is cancel-safe, so losing the race to the
                        // deadline cannot consume a chunk.
                        let received = match deadline {
                            Some(limit) if limit <= Instant::now() => return Chunks::Idle,
                            Some(limit) => {
                                match tokio::time::timeout_at(limit, connection.rx.recv()).await {
                                    Ok(received) => received,
                                    Err(_) => return Chunks::Idle,
                                }
                            }
                            None => connection.rx.recv().await,
                        };
                        if let Some(chunk) = received {
                            return Chunks::Got(vec![chunk]);
                        }
                    }
                    // The reader ended: the stream closed, errored or stalled.
                    self.end_connection().await;
                }
            }
        }
    }

    /// Decode one chunk from [`Self::next_chunks`] into stream events.
    ///
    /// `Err` is a stream whose framing is gone — a header that never
    /// terminates, where there is nothing left to resync on. The connection
    /// is given up and the next [`Self::next_chunks`] reconnects; the caller
    /// should drop whatever else it took from this connection.
    pub(crate) fn decode(&mut self, chunk: &Bytes) -> Result<Vec<StreamEvent>> {
        let decoded = self.decoder.decode(chunk);
        if let Err(e) = &decoded {
            warn!("HTTP stream lost its framing ({e}); reconnecting");
            if let Link::Up(_) = self.link {
                let Link::Up(connection) = std::mem::replace(&mut self.link, Link::Finished) else {
                    unreachable!("matched above");
                };
                self.link = Link::Broken(connection);
            }
        }
        decoded
    }

    /// One connection attempt: the writes the owner asked for, then the open.
    /// On failure the next attempt is scheduled, spending budget.
    async fn attempt(&mut self) -> Result<()> {
        let reconnecting = std::mem::replace(&mut self.attempted, true);
        if reconnecting
            && !self.listen_only()
            && let Some(tuning) = &self.retune_on_reconnect
        {
            // Best effort: a wrongly tuned stream is still more useful than
            // none, and the next attempt tries again.
            let desired = *tuning.lock().unwrap_or_else(|p| p.into_inner());
            if let Err(e) = self
                .control_client
                .configure_capture(desired.control())
                .await
            {
                warn!("Re-applying tuning before reconnecting failed: {e}");
            }
        }
        if self.streaming_control && !self.listen_only() {
            match self.control_client.control_streaming(true).await {
                Ok(()) => info!("Started streaming via control endpoint"),
                Err(e) => {
                    debug!("Could not control streaming (device may already be streaming): {e}")
                }
            }
        }
        match self.stream_client.open_stream(&self.params).await {
            Ok(response) => {
                let (tx, rx) = tokio::sync::mpsc::channel(CHUNK_CHANNEL_DEPTH);
                let reader = tokio::spawn(read_stream_into(
                    response.bytes_stream(),
                    tx,
                    STREAM_STALL_TIMEOUT,
                ));
                self.link = Link::Up(Connection {
                    rx,
                    reader: Some(reader),
                    opened: Instant::now(),
                });
                self.connections += 1;
                Ok(())
            }
            Err(e) => {
                warn!("Could not open the HTTP stream: {e}");
                self.retry_later();
                Err(e)
            }
        }
    }

    /// End the current connection and schedule the next one.
    async fn end_connection(&mut self) {
        let healthy = match std::mem::replace(&mut self.link, Link::Finished) {
            Link::Up(connection) | Link::Broken(connection) => {
                // Released now rather than after the backoff: a stalled
                // connection still holds a server slot.
                let healthy = connection.opened.elapsed() >= self.backoff.policy.healthy_after;
                drop(connection);
                self.after_disconnect().await;
                healthy
            }
            other => {
                self.link = other;
                return;
            }
        };
        if healthy {
            self.backoff.reset();
        }
        self.retry_later();
    }

    /// What every ended connection needs: the stop write, if the owner asked
    /// for it, and a decoder ready for a new packet sequence.
    async fn after_disconnect(&mut self) {
        if self.streaming_control && !self.listen_only() {
            match self.control_client.control_streaming(false).await {
                Ok(()) => info!("Stopped streaming via control endpoint"),
                Err(e) => debug!(
                    "Could not stop streaming via control endpoint (device may handle this \
                     automatically): {e}"
                ),
            }
        }
        self.decoder.connection_replaced();
    }

    /// Schedule the next attempt, or finish once the budget is spent.
    fn retry_later(&mut self) {
        self.link = match self.backoff.next() {
            Some(delay) => {
                let attempt = self.backoff.spent;
                match self.backoff.policy.attempts {
                    Some(allowed) => warn!(
                        "HTTP stream lost; reconnecting in {delay:?} (attempt {attempt}/{allowed})"
                    ),
                    None => {
                        warn!("HTTP stream lost; reconnecting in {delay:?} (attempt {attempt})")
                    }
                }
                Link::Down {
                    due: Instant::now()
                        .checked_add(delay)
                        .unwrap_or_else(Instant::now),
                }
            }
            None => Link::Finished,
        };
    }
}

/// What only the FutureSDR block uses: an explicit first connection, a
/// restart on request, and the state its statistics report.
#[cfg(feature = "futuresdr")]
impl HttpStreamEngine {
    /// Open the first connection now, reporting why it failed. A failure
    /// leaves the next attempt scheduled, as any failed attempt does.
    pub(crate) async fn connect(&mut self) -> Result<()> {
        self.attempt().await
    }

    /// End the current connection, if any — with the stop write, when the
    /// owner asked for one — and leave the next attempt due at once.
    pub(crate) async fn disconnect(&mut self) {
        if let Link::Up(_) | Link::Broken(_) = self.link {
            self.link = Link::Finished; // drops the connection
            self.after_disconnect().await;
        }
        self.link = Link::Down {
            due: Instant::now(),
        };
    }

    /// Replace the connection on request, for a retune that only a new
    /// connection picks up: [`Self::disconnect`], with the full reconnect
    /// budget restored. The next [`Self::next_chunks`] reconnects.
    pub(crate) async fn restart(&mut self) {
        self.disconnect().await;
        self.backoff.reset();
    }

    /// Select the server's named input for every later connection.
    pub(crate) fn set_input(&mut self, input: String) {
        self.params.input = Some(input);
    }

    pub(crate) fn set_listen_only(&self, on: bool) {
        self.listen_only.store(on, Ordering::Relaxed);
    }

    pub(crate) fn is_listen_only(&self) -> bool {
        self.listen_only()
    }

    /// Whether a connection is open.
    pub(crate) fn is_connected(&self) -> bool {
        matches!(self.link, Link::Up(_) | Link::Broken(_))
    }

    /// Connections opened so far, and when the latest one was. A change is
    /// how an owner learns the engine reconnected.
    pub(crate) fn connections(&self) -> (u64, Option<std::time::Instant>) {
        let opened = match &self.link {
            Link::Up(connection) | Link::Broken(connection) => Some(connection.opened.into_std()),
            _ => None,
        };
        (self.connections, opened)
    }

    /// The parser's throughput counters.
    pub(crate) fn parser_stats(&self) -> crate::http_streaming::StreamingPerformanceStats {
        self.decoder.parser.stats()
    }

    /// Server gaps the drop detector has counted, across every connection.
    pub(crate) fn drops(&self) -> u64 {
        self.decoder.drops.drops()
    }
}

#[cfg(test)]
impl HttpStreamEngine {
    /// Feed chunks from `rx` as if a connection had delivered them.
    pub(crate) fn attach(&mut self, rx: tokio::sync::mpsc::Receiver<Bytes>) {
        self.link = Link::Up(Connection {
            rx,
            reader: None,
            opened: Instant::now(),
        });
    }

    /// The URL the next connection opens. Only `HttpSource`'s tests ask.
    #[cfg(feature = "futuresdr")]
    pub(crate) fn stream_url(&self) -> String {
        self.params.stream_url(self.stream_client.base_url())
    }
}

/// Drain `stream` into `tx` until it ends, errors, delivers nothing for
/// `stall`, or the receiver is dropped. Dropping `tx` on return is what tells
/// the engine to reconnect.
///
/// A task of its own so the socket is read continuously, whatever the
/// owner's polling rate. Read one chunk per FutureSDR `work()` call, the
/// stream was measured at a tenth of what `curl` pulls from the same
/// endpoint: between calls the socket was not read and the server's stream
/// backed up.
async fn read_stream_into<S, E>(stream: S, tx: tokio::sync::mpsc::Sender<Bytes>, stall: Duration)
where
    S: futures::Stream<Item = std::result::Result<Bytes, E>>,
    E: std::fmt::Display,
{
    use futures::StreamExt;
    let mut stream = std::pin::pin!(stream);
    loop {
        match tokio::time::timeout(stall, stream.next()).await {
            Ok(Some(Ok(chunk))) => {
                // `send` awaits when the channel is full — this is the
                // backpressure point, and where TCP flow control kicks in.
                // A send error means the receiver was dropped, so the
                // engine is gone and the task should end.
                if tx.send(chunk).await.is_err() {
                    break;
                }
            }
            Ok(Some(Err(e))) => {
                warn!("Stream chunk error in reader task: {e}");
                break;
            }
            Ok(None) => {
                info!("HTTP stream ended");
                break;
            }
            Err(_) => {
                warn!(
                    "HTTP stream delivered nothing for {:.1} s; treating the connection as lost",
                    stall.as_secs_f64()
                );
                break;
            }
        }
    }
    debug!("HTTP stream reader task ended");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_endpoints::AuthMethod;
    use crate::http_streaming::StreamFormat;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// One int16 IQ packet as the wire carries it: JSON header, the live
    /// server's LF+RS separator, then `pairs` zeroed IQ pairs.
    fn iq_packet(start: f64, end: f64, pairs: usize) -> Vec<u8> {
        let mut out = format!(
            r#"{{"startTime":{start},"endTime":{end},"startFrequency":95e6,"endFrequency":105e6,"sampleFrequency":15360000,"samples":{pairs},"unit":"volt","payload":"iq","minPower":-120,"maxPower":0,"sampleSize":2}}"#
        )
        .into_bytes();
        out.extend_from_slice(&[0x0A, 0x1E]);
        out.extend(std::iter::repeat_n(0u8, pairs * 4));
        out
    }

    /// One int16 spectra frame of `bins` scalars, without the
    /// `sampleFrequency` the live server leaves off them.
    fn spectra_packet(start: f64, end: f64, bins: usize) -> Vec<u8> {
        let mut out = format!(
            r#"{{"startTime":{start},"endTime":{end},"startFrequency":95e6,"endFrequency":105e6,"samples":1,"unit":"dbm","payload":"spectra","minPower":-120,"maxPower":0,"sampleSize":{bins}}}"#
        )
        .into_bytes();
        out.extend_from_slice(&[0x0A, 0x1E]);
        out.extend(std::iter::repeat_n(0u8, bins * 2));
        out
    }

    fn int16_params() -> StreamParams {
        HttpEndpointsClient::stream_params()
            .format(StreamFormat::Int16)
            .build()
    }

    fn kinds(events: &[StreamEvent]) -> Vec<&'static str> {
        events
            .iter()
            .map(|event| match event {
                StreamEvent::Iq(_) => "iq",
                StreamEvent::NonIq { .. } => "non-iq",
                StreamEvent::Break(StreamGap::Reconnect) => "reconnect",
                StreamEvent::Break(StreamGap::Server { .. }) => "server",
                StreamEvent::Break(StreamGap::Backward { .. }) => "backward",
                StreamEvent::Break(StreamGap::Lost(_)) => "lost",
            })
            .collect()
    }

    const FAST: ReconnectPolicy = ReconnectPolicy {
        first_delay: Duration::from_millis(10),
        max_delay: Duration::from_millis(10),
        attempts: None,
        healthy_after: Duration::from_secs(30),
    };

    fn engine_for(
        uri: &str,
        reconnect: ReconnectPolicy,
        listen_only: bool,
        retune: bool,
        streaming_control: bool,
    ) -> HttpStreamEngine {
        let client = HttpEndpointsClient::new(uri.to_string(), AuthMethod::None).unwrap();
        HttpStreamEngine::new(EngineOptions {
            stream_client: client.clone(),
            control_client: client,
            params: int16_params(),
            listen_only: Arc::new(AtomicBool::new(listen_only)),
            reconnect,
            retune_on_reconnect: retune.then(|| {
                Arc::new(Mutex::new(CaptureTuning {
                    center_frequency_hz: 915e6,
                    sample_rate_hz: 15.36e6,
                    reference_level_dbm: -20.0,
                }))
            }),
            streaming_control,
        })
        .unwrap()
    }

    /// Drive `engine` until `want` IQ packets have been decoded, returning
    /// every event on the way.
    async fn decode_until(engine: &mut HttpStreamEngine, want: usize) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while events
            .iter()
            .filter(|e| matches!(e, StreamEvent::Iq(_)))
            .count()
            < want
        {
            assert!(
                std::time::Instant::now() < deadline,
                "only {:?} arrived",
                kinds(&events)
            );
            if let Chunks::Got(chunks) = engine.next_chunks(Some(Duration::from_secs(1))).await {
                for chunk in &chunks {
                    events.extend(engine.decode(chunk).expect("whole packets decode"));
                }
            }
        }
        events
    }

    async fn requests(server: &MockServer) -> Vec<(String, String, serde_json::Value)> {
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .map(|r| {
                (
                    r.method.to_string(),
                    r.url.path().to_string(),
                    serde_json::from_slice(&r.body).unwrap_or(serde_json::Value::Null),
                )
            })
            .collect()
    }

    /// The SpectranSource schedule: 250 ms doubling to 4 s, five attempts,
    /// then nothing — and a reset restores all of it.
    #[test]
    fn the_backoff_doubles_to_its_ceiling_and_spends_its_budget() {
        let mut backoff = Backoff::new(ReconnectPolicy {
            first_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(4),
            attempts: Some(5),
            healthy_after: Duration::from_secs(30),
        });
        let delays: Vec<_> = std::iter::from_fn(|| backoff.next()).collect();
        assert_eq!(
            delays,
            [250, 500, 1000, 2000, 4000].map(Duration::from_millis),
        );
        assert_eq!(backoff.next(), None, "a spent budget stays spent");
        backoff.reset();
        assert_eq!(backoff.next(), Some(Duration::from_millis(250)));

        // No budget: the FutureSDR block's fixed second, for ever.
        let mut forever = Backoff::new(ReconnectPolicy {
            attempts: None,
            first_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(1),
            ..FAST
        });
        assert!((0..1000).all(|_| forever.next() == Some(Duration::from_secs(1))));

        // A zero budget never reconnects at all.
        let mut never = Backoff::new(ReconnectPolicy {
            attempts: Some(0),
            ..FAST
        });
        assert_eq!(never.next(), None);
    }

    /// A packet the parser cannot decode is a break where it sat, and the
    /// drop detector is re-seeded so its hole is not counted again as a
    /// server gap when the next packet's timestamp shows it.
    #[test]
    fn a_lost_packet_is_a_break_not_a_server_gap() {
        let mut decoder = StreamDecoder::new(&int16_params()).unwrap();
        let mut wire = iq_packet(100.000, 100.001, 16);
        // Frames, but its payload cannot be sized.
        wire.extend(
            format!(
                r#"{{"startTime":100.001,"endTime":100.002,"startFrequency":95e6,"endFrequency":105e6,"sampleFrequency":15360000,"samples":{},"unit":"volt","payload":"iq","minPower":-120,"maxPower":0,"sampleSize":2}}"#,
                u64::MAX
            )
            .into_bytes(),
        );
        wire.extend_from_slice(&[0x0A, 0x1E]);
        wire.extend(iq_packet(100.002, 100.003, 16));

        let events = decoder
            .decode(&Bytes::from(wire))
            .expect("a loss is not an error");
        assert_eq!(kinds(&events), ["iq", "lost", "iq"]);
        assert_eq!(
            decoder.drops.drops(),
            0,
            "the hole is ours, not a server skip"
        );
    }

    /// Spectra skip the IQ path entirely: neither a break, nor fed to the
    /// drop detector — interleaved with IQ, each would read as a jump in
    /// the IQ timeline. One the parser cannot decode (a compressed frame)
    /// says why.
    #[test]
    fn non_iq_packets_are_skipped_without_a_break() {
        let mut decoder = StreamDecoder::new(&int16_params()).unwrap();
        let mut wire = iq_packet(100.000, 100.001, 16);
        wire.extend(spectra_packet(100.0005, 100.0105, 8));
        wire.extend_from_slice(
            br#"{"startTime":100.0005,"endTime":100.0105,"startFrequency":95e6,"endFrequency":105e6,"samples":4,"unit":"dbm","payload":"spectra","minPower":-120,"maxPower":0,"sampleSize":64,"sampleDepth":1,"compression":3}"#,
        );
        wire.extend_from_slice(&[0x0A, 0x1E]);
        wire.extend_from_slice(&[0x93, b'{', 0x0A, 0x1E, 0xA5, 0x55, 0x00, 0xFF]);
        wire.extend(iq_packet(100.001, 100.002, 16));

        let events = decoder.decode(&Bytes::from(wire)).expect("decodes");
        assert_eq!(kinds(&events), ["iq", "non-iq", "non-iq", "iq"]);
        assert!(matches!(
            &events[1],
            StreamEvent::NonIq {
                payload: PayloadType::Spectra,
                undecoded: None
            }
        ));
        assert!(matches!(
            &events[2],
            StreamEvent::NonIq {
                payload: PayloadType::Spectra,
                undecoded: Some(_)
            }
        ));
        assert_eq!(decoder.drops.drops(), 0);
    }

    /// The drop detector's two findings arrive in stream order, ahead of
    /// the packet they precede, carrying the running totals.
    #[test]
    fn server_gaps_and_backward_steps_precede_their_packet() {
        let mut decoder = StreamDecoder::new(&int16_params()).unwrap();
        let mut wire = iq_packet(100.000, 100.001, 16);
        // 10 ms the server never sent, then 10 ms back.
        wire.extend(iq_packet(100.011, 100.012, 16));
        wire.extend(iq_packet(100.002, 100.003, 16));
        let events = decoder.decode(&Bytes::from(wire)).expect("decodes");
        assert_eq!(kinds(&events), ["iq", "server", "iq", "backward", "iq"]);
        assert!(matches!(
            events[1],
            StreamEvent::Break(StreamGap::Server { drops: 1, gap_seconds }) if gap_seconds > 0.009
        ));
        assert!(matches!(
            events[3],
            StreamEvent::Break(StreamGap::Backward { steps: 1, .. })
        ));
    }

    /// Across a new connection the partial packet from the old one is gone,
    /// the first IQ packet is preceded by a reconnect break and nothing
    /// else, and the timestamp jump the outage leaves is not a server gap.
    #[test]
    fn a_replaced_connection_starts_clean_behind_one_break() {
        let mut decoder = StreamDecoder::new(&int16_params()).unwrap();
        let events = decoder
            .decode(&Bytes::from(iq_packet(100.000, 100.001, 16)))
            .unwrap();
        assert_eq!(kinds(&events), ["iq"]);
        // Half a packet, then the connection goes.
        let half = iq_packet(100.001, 100.002, 16);
        assert!(
            decoder
                .decode(&Bytes::copy_from_slice(&half[..40]))
                .unwrap()
                .is_empty()
        );
        decoder.connection_replaced();

        let mut wire = spectra_packet(200.0, 200.01, 4);
        wire.extend(iq_packet(200.000, 200.001, 16));
        wire.extend(iq_packet(200.001, 200.002, 16));
        let events = decoder.decode(&Bytes::from(wire)).unwrap();
        assert_eq!(
            kinds(&events),
            ["non-iq", "reconnect", "iq", "iq"],
            "the old header must not be applied to the new stream's bytes"
        );
        assert_eq!(decoder.drops.drops(), 0, "an outage is not a server gap");
    }

    /// One call takes at most a channel's depth of chunks, however many are
    /// queued — the bound a caller sizes its buffers by. The rest wait.
    #[tokio::test]
    async fn one_call_takes_at_most_one_channel_depth() {
        let mut engine = engine_for("http://localhost:54664", FAST, false, false, false);
        let (tx, rx) = tokio::sync::mpsc::channel(CHUNK_CHANNEL_DEPTH * 2);
        for _ in 0..CHUNK_CHANNEL_DEPTH + 10 {
            tx.try_send(Bytes::new()).unwrap();
        }
        engine.attach(rx);

        let Chunks::Got(first) = engine.next_chunks(Some(Duration::ZERO)).await else {
            panic!("chunks were queued");
        };
        assert_eq!(first.len(), CHUNK_CHANNEL_DEPTH);
        let Chunks::Got(rest) = engine.next_chunks(Some(Duration::ZERO)).await else {
            panic!("ten were left");
        };
        assert_eq!(rest.len(), 10);
        drop(tx);
    }

    /// A stream whose framing is gone gives its connection up: the next
    /// call ends it and schedules a reconnect, and the decoder starts the
    /// next connection clean.
    #[tokio::test]
    async fn lost_framing_gives_the_connection_up() {
        let mut engine = engine_for("http://localhost:54664", FAST, false, false, false);
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        engine.attach(rx);
        engine
            .decode(&Bytes::from(iq_packet(100.0, 100.001, 4)))
            .unwrap();
        // A header that never terminates: nothing left to resync on.
        let mut garbage = vec![b'{'];
        garbage.resize(10 * 1024 * 1024 + 2, b'x');
        let garbage = Bytes::from(garbage);
        assert!(engine.decode(&garbage).is_err());
        assert!(matches!(engine.link, Link::Broken(_)));

        assert!(matches!(
            engine.next_chunks(Some(Duration::ZERO)).await,
            Chunks::Idle
        ));
        assert!(
            matches!(engine.link, Link::Down { .. }),
            "a reconnect is scheduled"
        );
        let events = engine
            .decode(&Bytes::from(iq_packet(200.0, 200.001, 4)))
            .unwrap();
        assert_eq!(kinds(&events), ["reconnect", "iq"]);
        drop(tx);
    }

    /// A stream that ends is reopened after the policy's delay. Before the
    /// reconnect the owner's tuning goes out as a full `/control` capture
    /// tuple, and each connection is bracketed by the streaming start and
    /// stop the owner asked for.
    #[tokio::test]
    async fn a_lost_stream_reconnects_with_the_owners_writes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/stream"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(iq_packet(100.0, 100.001, 4)))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/control"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        let mut engine = engine_for(&server.uri(), FAST, false, true, true);
        let events = decode_until(&mut engine, 2).await;
        assert_eq!(kinds(&events), ["iq", "reconnect", "iq"]);

        let requests = requests(&server).await;
        let order: Vec<&str> = requests
            .iter()
            .map(|(m, p, body)| match (m.as_str(), p.as_str()) {
                ("GET", "/stream") => "open",
                ("PUT", "/control") if body.get("start") == Some(&true.into()) => "start",
                ("PUT", "/control") if body.get("start") == Some(&false.into()) => "stop",
                ("PUT", "/control")
                    if body["frequencyCenter"].as_f64() == Some(915e6)
                        && body["frequencySpan"].as_f64() == Some(15.36e6) =>
                {
                    "retune"
                }
                _ => "other",
            })
            .collect();
        assert_eq!(
            &order[..6],
            ["start", "open", "stop", "retune", "start", "open"],
            "{requests:?}"
        );
    }

    /// Listening only, nothing is written: not the re-tune, not the
    /// streaming switches, on the first connection or any reconnect.
    #[tokio::test]
    async fn listen_only_writes_nothing_across_reconnects() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/stream"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(iq_packet(100.0, 100.001, 4)))
            .mount(&server)
            .await;

        let mut engine = engine_for(&server.uri(), FAST, true, true, true);
        decode_until(&mut engine, 3).await;
        let requests = requests(&server).await;
        assert!(requests.len() >= 3, "three connections: {requests:?}");
        assert!(
            requests
                .iter()
                .all(|(m, p, _)| m == "GET" && p == "/stream"),
            "listening only, nothing is written: {requests:?}"
        );
    }

    /// Every failure spends one attempt — a failed open as much as a lost
    /// connection — and a spent budget finishes the stream.
    #[tokio::test]
    async fn a_spent_budget_finishes_the_stream() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/stream"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let mut engine = engine_for(
            &server.uri(),
            ReconnectPolicy {
                attempts: Some(2),
                ..FAST
            },
            false,
            false,
            false,
        );
        let finished = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Chunks::Finished = engine.next_chunks(None).await {
                    break;
                }
            }
        })
        .await;
        assert!(
            finished.is_ok(),
            "the engine must give up, not retry for ever"
        );
        let opens = requests(&server).await.len();
        assert_eq!(opens, 3, "the first attempt and two reconnects");
    }

    /// A restart replaces the connection at once — with the stop write and
    /// a full budget — and the new connection's first packet carries the
    /// break.
    #[cfg(feature = "futuresdr")]
    #[tokio::test]
    async fn a_restart_reconnects_at_once() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/stream"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(iq_packet(100.0, 100.001, 4)))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/control"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        // A reconnect delay far longer than the test: only a restart can
        // reopen the stream in time.
        let slow = ReconnectPolicy {
            first_delay: Duration::from_secs(60),
            max_delay: Duration::from_secs(60),
            ..FAST
        };
        let mut engine = engine_for(&server.uri(), slow, false, false, true);
        engine.connect().await.expect("the mock stream opens");
        assert_eq!(engine.connections().0, 1);
        assert_eq!(kinds(&decode_until(&mut engine, 1).await), ["iq"]);

        engine.restart().await;
        assert!(!engine.is_connected());
        let events = decode_until(&mut engine, 1).await;
        assert_eq!(kinds(&events), ["reconnect", "iq"]);
        assert_eq!(engine.connections().0, 2);
        let stops = requests(&server)
            .await
            .iter()
            .filter(|(_, _, body)| body.get("start") == Some(&false.into()))
            .count();
        assert_eq!(stops, 1, "the restart wrote the stop");
    }

    /// A server that stops sending while holding the socket open must not
    /// park the reader forever: it gives up after the stall timeout and
    /// drops its sender, which is what sends the engine down its
    /// reconnect path.
    #[tokio::test]
    async fn a_stalled_stream_ends_the_reader() {
        use futures::StreamExt;
        let chunks =
            futures::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from_static(b"one"))])
                .chain(futures::stream::pending());
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let reader = tokio::spawn(read_stream_into(chunks, tx, Duration::from_millis(50)));

        assert_eq!(rx.recv().await, Some(Bytes::from_static(b"one")));
        let ended = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("the stalled reader must give up, not hang");
        assert_eq!(ended, None, "the channel closes: the engine reconnects");
        reader.await.unwrap();
    }
}
