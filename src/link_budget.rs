//! Will this link carry the span you asked for?
//!
//! An RTSA server streams IQ over HTTP at a fixed number of bytes per
//! sample, and the span the operator asks for picks the sample rate off
//! the decimation ladder in [`crate::utils`]. Multiply the two and you
//! have a byte rate the path — server, network, and this host's ingest —
//! has to sustain. Miss it and the *server* drops what it cannot send,
//! which is worse than it sounds: each gap is an unsignalled
//! discontinuity, so a capture that looks fine in a waterfall is
//! undecodable to any digital demodulator.
//!
//! Measured against a SPECTRAN V6 ECO over a gigabit path:
//!
//! | `--span` | sample rate | needs      | result                            |
//! |----------|-------------|------------|-----------------------------------|
//! | 10 MHz   | 15.36 MS/s  | 61.4 MB/s  | zero gaps, contiguous stream      |
//! | 20 MHz   | 30.72 MS/s  | 122.9 MB/s | 1024 skips, 1.84 s lost of 35 s   |
//!
//! `DropDetector` reports the second row *after* the capture is already
//! corrupted. This module is the predictive half: measure what the path
//! delivers, and name the widest span that fits it.
//!
//! ```
//! use sdr_aaronia_rs::link_budget::{max_sustainable_span, required_byte_rate};
//!
//! // 15.36 MS/s at 4 bytes a sample.
//! assert_eq!(required_byte_rate(15_360_000.0), 61_440_000.0);
//!
//! // A path measured at 75 MB/s (WiFi 7, station to station) carries the
//! // 15.36 MS/s rung and nothing above it: 12.288 MHz of usable span.
//! assert_eq!(max_sustainable_span(75_000_000.0), 12_288_000.0);
//! ```
//!
//! The end-to-end probe is [`measure_link_throughput`]. Deliberately
//! *end to end*: the bottleneck may be the server, a switch, the air, or
//! this host, and only bytes counted off the socket see all of them. The
//! NIC's advertised link speed sees none of them.

use std::time::{Duration, Instant};

use crate::http_endpoints::AuthMethod;
use crate::http_streaming::{StreamFormat, StreamParser};
use crate::utils::{iq_sample_rates, usable_bandwidth_hz};
use crate::{Error, Result};

use tracing::{debug, info};

/// The wire format the byte-rate helpers assume when none is named:
/// `int16`, 4 bytes a sample.
///
/// This is [`crate::HttpSourceBuilder`]'s default and what the live
/// capture path streams, so it is the format whose arithmetic an
/// operator is actually asking about. Use the `_for_format` variants for
/// anything else — `float32` is 8 bytes a sample and needs twice the
/// link.
pub const DEFAULT_LINK_FORMAT: StreamFormat = StreamFormat::Int16;

/// How much of a freshly-opened stream to throw away before counting.
///
/// **A probe without this lies.** The RTSA server hands over its
/// pre-connect backlog at connect, faster than real time, so the first
/// fraction of a second arrives at whatever rate the socket can be
/// filled rather than at the rate the device produces. Count it and the
/// measurement comes out *above* the true link rate, and a span that
/// cannot fit is pronounced fine.
///
/// Measured (commit f322c60, live at 15.4 MS/s): the residual 4.19 M
/// dropped samples of a 45 s run all fell inside a **345 ms** window at
/// connect, with zero for the remaining 43 s; the backlog itself is
/// ~0.27–0.35 s of signal. 500 ms clears that with margin and costs
/// nothing but half a second of a probe.
pub const LINK_PROBE_SETTLE: Duration = Duration::from_millis(500);

/// Longest the probe waits for the next chunk before calling the stream
/// dead.
///
/// At any real rate the server sends ~157 KiB chunks many times a
/// second, so five seconds of silence is not slow streaming — it is a
/// mission that is not running, or a connection that has hung. Reporting
/// that as "0 MB/s" would turn a failed measurement into a verdict that
/// every span is too wide, which is exactly the mistake this module
/// exists to prevent.
const PROBE_STALL_TIMEOUT: Duration = Duration::from_secs(5);

/// How many chunks the probe will parse looking for a packet header.
///
/// It wants one number out of the stream — the rate the device is
/// running at, which is what tells a caller whether the path was
/// actually loaded — and that is in the first header. The server sends
/// ~157 KiB chunks, so eight is over a megabyte: far more than one
/// packet, and a hard stop for a stream that turns out not to be this
/// protocol at all.
const PROBE_HEADER_CHUNKS: u32 = 8;

/// Bytes a second the path must sustain to carry `sample_rate_hz` in
/// `format`.
///
/// The floor, not the total: each packet also carries a JSON metadata
/// header, so a healthy stream measures slightly *above* this. That
/// asymmetry is deliberate — a comparison against this figure cannot
/// fire on protocol overhead.
///
/// [`StreamFormat::Json`] has no fixed size per sample and returns 0.0,
/// meaning "no budget can be computed", not "free".
pub fn required_byte_rate_for_format(sample_rate_hz: f64, format: StreamFormat) -> f64 {
    if !sample_rate_hz.is_finite() || sample_rate_hz <= 0.0 {
        return 0.0;
    }
    sample_rate_hz * format.iq_bytes_per_sample() as f64
}

/// Bytes a second the path must sustain to carry `sample_rate_hz` in the
/// [`DEFAULT_LINK_FORMAT`] (4 bytes a sample).
pub fn required_byte_rate(sample_rate_hz: f64) -> f64 {
    required_byte_rate_for_format(sample_rate_hz, DEFAULT_LINK_FORMAT)
}

/// The fastest rung of the decimation ladder whose stream fits
/// `measured_byte_rate_hz`, in samples a second.
///
/// Inverts [`required_byte_rate_for_format`] through
/// [`crate::utils::iq_sample_rates`] rather than solving for a rate and
/// rounding: the device has no rates between the rungs, so the only
/// useful answer is one the hardware can actually be set to.
///
/// Returns 0.0 when even the slowest rung (120 kS/s, 480 kB/s) does not
/// fit — a link that narrow is the problem, not the span.
pub fn max_sustainable_sample_rate_for_format(
    measured_byte_rate_hz: f64,
    format: StreamFormat,
) -> f64 {
    if !measured_byte_rate_hz.is_finite() || measured_byte_rate_hz <= 0.0 {
        return 0.0;
    }
    iq_sample_rates()
        .into_iter()
        .filter(|rate| {
            let needed = required_byte_rate_for_format(*rate, format);
            needed > 0.0 && needed <= measured_byte_rate_hz
        })
        // The ladder is ordered fastest-first, so the first match is the
        // answer; `fold` over `max` rather than `next()` keeps this
        // correct if that order ever changes.
        .fold(0.0f64, f64::max)
}

/// The widest span on the decimation ladder that fits
/// `measured_byte_rate_hz`, in Hz, for the [`DEFAULT_LINK_FORMAT`].
///
/// "Span" here is the usable (alias-free) bandwidth the rung delivers —
/// the same quantity `--span` selects, so the answer can be handed
/// straight back to the operator. Round-trips: feeding it to
/// [`crate::utils::iq_sample_rate_for_bandwidth`] returns the rung it
/// came from.
///
/// Returns 0.0 when nothing on the ladder fits.
pub fn max_sustainable_span(measured_byte_rate_hz: f64) -> f64 {
    max_sustainable_span_for_format(measured_byte_rate_hz, DEFAULT_LINK_FORMAT)
}

/// The widest span on the decimation ladder that fits
/// `measured_byte_rate_hz`, in Hz, for `format`.
pub fn max_sustainable_span_for_format(measured_byte_rate_hz: f64, format: StreamFormat) -> f64 {
    let rate = max_sustainable_sample_rate_for_format(measured_byte_rate_hz, format);
    if rate <= 0.0 {
        return 0.0;
    }
    usable_bandwidth_hz(rate)
}

/// What a throughput measurement actually saw.
///
/// A bare number would let a caller report a verdict but not the
/// evidence, and the evidence is the point: a rate is only meaningful
/// alongside the window it was averaged over and the settle period that
/// was thrown away first.
#[derive(Debug, Clone, PartialEq)]
pub struct ThroughputMeasurement {
    /// Sustained bytes a second over the counted window.
    pub byte_rate: f64,
    /// Bytes counted (excludes everything discarded during settle).
    pub bytes: u64,
    /// Wall clock the counted window actually covered — not the window
    /// that was requested, which chunk boundaries rarely land on.
    pub window: Duration,
    /// Settle period discarded before counting began; see
    /// [`LINK_PROBE_SETTLE`] for why it exists.
    pub settle: Duration,
    /// Bytes that arrived during the settle period and were discarded.
    /// Reported rather than hidden: at a wide span this is the
    /// pre-connect backlog, and its size is itself diagnostic.
    pub settle_bytes: u64,
    /// The rate the device was streaming at while this was measured,
    /// read from a packet header — `None` when no header was parsed.
    ///
    /// This is the yardstick for whether the path was *loaded*. A
    /// measurement of 4 MB/s taken while the device streamed 1 MS/s says
    /// nothing about a gigabit link; the same 4 MB/s while the device
    /// streamed 15.36 MS/s says the path is failing badly.
    pub stream_sample_rate: Option<f64>,
}

impl ThroughputMeasurement {
    /// The measured rate in MB/s, the unit link budgets are argued in.
    pub fn megabytes_per_second(&self) -> f64 {
        self.byte_rate / 1e6
    }

    /// The widest span this measurement proves the path can carry, in Hz
    /// — see [`max_sustainable_span`], and note "proves": if the device
    /// was streaming narrower than the link could carry, this is a floor
    /// on the answer rather than the answer.
    pub fn max_sustainable_span_hz(&self) -> f64 {
        max_sustainable_span(self.byte_rate)
    }
}

impl std::fmt::Display for ThroughputMeasurement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:.1} MB/s sustained over {:.2} s (after discarding a {:.2} s settle window \
             carrying {:.1} MB)",
            self.megabytes_per_second(),
            self.window.as_secs_f64(),
            self.settle.as_secs_f64(),
            self.settle_bytes as f64 / 1e6,
        )
    }
}

/// Counts bytes against a wall clock, discarding an initial settle
/// window.
///
/// Split out from [`measure_link_throughput`] for two reasons: the live
/// [`crate::http_source::HttpSource`] runs the same check passively on
/// the stream it is already reading rather than opening a second
/// connection, and the settle logic — the part a naive implementation
/// gets wrong — is testable here against synthetic timestamps instead of
/// a real socket.
#[derive(Debug, Clone)]
pub struct ThroughputMeter {
    settle: Duration,
    window: Duration,
    opened: Instant,
    /// First observation after the settle period. Counting runs from
    /// here, and this observation's own bytes are *not* counted — see
    /// [`Self::observe`].
    mark: Option<Instant>,
    last: Option<Instant>,
    discarded_bytes: u64,
    counted_bytes: u64,
    complete: bool,
}

impl ThroughputMeter {
    /// Start measuring now: discard `settle`, then count for `window`.
    pub fn new(settle: Duration, window: Duration) -> Self {
        Self::starting_at(Instant::now(), settle, window)
    }

    /// As [`Self::new`], but with the stream-open instant supplied.
    ///
    /// Lets a caller anchor the settle window on the moment the response
    /// headers arrived rather than on whenever the meter was built, and
    /// lets tests drive the whole state machine off synthetic instants
    /// with no sleeping and no flakiness.
    pub fn starting_at(opened: Instant, settle: Duration, window: Duration) -> Self {
        Self {
            settle,
            window,
            opened,
            mark: None,
            last: None,
            discarded_bytes: 0,
            counted_bytes: 0,
            complete: false,
        }
    }

    /// Record `bytes` received at `at`. Returns whether the measurement
    /// is complete.
    ///
    /// Bytes before `opened + settle` are discarded, for the reason
    /// [`LINK_PROBE_SETTLE`] documents. So are the bytes of the first
    /// observation past that boundary: they accumulated *before* it, and
    /// counting them against a window that starts at that instant is the
    /// same inflation in miniature. What is counted is exactly the bytes
    /// that arrived between the mark and the last observation, over
    /// exactly that interval.
    pub fn observe(&mut self, at: Instant, bytes: usize) -> bool {
        if self.complete {
            return true;
        }
        let bytes = bytes as u64;
        if at.saturating_duration_since(self.opened) < self.settle {
            self.discarded_bytes = self.discarded_bytes.saturating_add(bytes);
            return false;
        }
        match self.mark {
            None => {
                self.mark = Some(at);
                self.discarded_bytes = self.discarded_bytes.saturating_add(bytes);
            }
            Some(mark) => {
                self.counted_bytes = self.counted_bytes.saturating_add(bytes);
                self.last = Some(at);
                if at.saturating_duration_since(mark) >= self.window {
                    self.complete = true;
                }
            }
        }
        self.complete
    }

    /// Whether the counting window has closed.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// The measurement, or `None` if there is not enough of one.
    ///
    /// `None` is the honest answer when the stream delivered nothing
    /// after the settle period, or delivered a single observation and so
    /// spans no interval. A caller must not turn that into "0 MB/s" —
    /// see the constraint at the top of [`measure_link_throughput`].
    pub fn finish(&self) -> Option<ThroughputMeasurement> {
        let mark = self.mark?;
        let last = self.last?;
        let window = last.saturating_duration_since(mark);
        if window.is_zero() {
            return None;
        }
        Some(ThroughputMeasurement {
            byte_rate: self.counted_bytes as f64 / window.as_secs_f64(),
            bytes: self.counted_bytes,
            window,
            settle: self.settle,
            settle_bytes: self.discarded_bytes,
            stream_sample_rate: None,
        })
    }
}

/// Measure what the path to `base_url` actually delivers, by streaming
/// from it and counting bytes off the socket for `duration`.
///
/// The total cost is `LINK_PROBE_SETTLE + duration` plus connect time;
/// the settle period is discarded before counting starts, because the
/// server's connect backlog arrives faster than real time and would
/// measure the socket rather than the link.
///
/// **This measures what the path delivered, which is a floor on what it
/// can deliver.** If the device is tuned to 1 MS/s the answer is 4 MB/s
/// and says nothing about a gigabit link. To measure a ceiling, set the
/// device at or above the span being planned first, then read
/// [`ThroughputMeasurement::stream_sample_rate`] to confirm the path was
/// actually loaded.
///
/// Read-only: it opens `/stream` and nothing else. It does not start the
/// mission, retune, or change any device setting, so a server whose
/// mission is stopped produces an error rather than a verdict. It does
/// take a client connection for the duration, which on the free RTSA
/// licence is the only one — run it before the capture, not alongside
/// it.
///
/// # Errors
///
/// Unreachable server, non-success status, a stream that stops sending,
/// or a stream that ends before the window closes. A failed measurement
/// is always an error and never a rate — "the server was unreachable" and
/// "the link delivers 0 MB/s" must not be confusable, since the second
/// one condemns every span on the ladder.
pub async fn measure_link_throughput(
    base_url: &str,
    duration: Duration,
) -> Result<ThroughputMeasurement> {
    measure_link_throughput_with(base_url, duration, AuthMethod::None, DEFAULT_LINK_FORMAT).await
}

/// As [`measure_link_throughput`], with the authentication method and
/// wire format named.
///
/// Probe in the format the capture will use: `float32` is 8 bytes a
/// sample, so a measurement taken in `int16` is only half the story for
/// a `float32` capture.
pub async fn measure_link_throughput_with(
    base_url: &str,
    duration: Duration,
    auth: AuthMethod,
    format: StreamFormat,
) -> Result<ThroughputMeasurement> {
    // Same scheme check the source applies at construction: a probe that
    // followed a `file://` URL would be a very different program.
    let parsed = url::Url::parse(base_url)
        .map_err(|_| Error::Protocol(format!("Invalid base URL format: {base_url}")))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(Error::Protocol(format!(
                "Only HTTP/HTTPS URLs are allowed, got: {other}"
            )));
        }
    }

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .user_agent(crate::utils::user_agent())
        .tcp_nodelay(true)
        // Same as the endpoints client: no total timeout (it would cover
        // the whole streamed body) and HTTP/1.1 only, which is all RTSA
        // speaks.
        .http1_only()
        .build()?;

    let url = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("format", format.as_str());
        format!("{}/stream?{}", base_url, query.finish())
    };

    info!(
        "Measuring link throughput from {url}: discarding {:.2} s of connect backlog, \
         then counting for {:.2} s",
        LINK_PROBE_SETTLE.as_secs_f64(),
        duration.as_secs_f64(),
    );

    let request = match &auth {
        AuthMethod::Basic { username, password } => {
            client.get(&url).basic_auth(username, Some(password))
        }
        AuthMethod::Token { token } => client
            .get(&url)
            .header("Authorization", format!("RToken {token}")),
        AuthMethod::None => client.get(&url),
    };

    let mut response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::Http {
            status,
            context: format!("Link throughput probe of {url}"),
        });
    }

    let mut meter = ThroughputMeter::new(LINK_PROBE_SETTLE, duration);
    // Parsed only until the first packet completes, purely to learn what
    // rate the device is streaming at — without it a caller cannot tell
    // a saturated path from an idle one. Dropped immediately after, so
    // the probe is not paying to decode the samples it is timing.
    let mut parser = StreamParser::new(format, None).ok();
    let mut header_chunks_left = PROBE_HEADER_CHUNKS;
    let mut stream_sample_rate = None;
    let mut stream_ended = false;

    loop {
        let next = tokio::time::timeout(PROBE_STALL_TIMEOUT, response.chunk()).await;
        let chunk = match next {
            Err(_elapsed) => {
                return Err(Error::Protocol(format!(
                    "No data from {url} for {:.0} s. The measurement was not taken — check that \
                     the RTSA mission is running and streaming on this input.",
                    PROBE_STALL_TIMEOUT.as_secs_f64(),
                )));
            }
            Ok(Err(e)) => return Err(Error::Transport(e)),
            Ok(Ok(None)) => {
                stream_ended = true;
                break;
            }
            Ok(Ok(Some(chunk))) => chunk,
        };

        if let Some(p) = parser.as_mut() {
            if let Ok(packets) = p.process_data(&chunk)
                && let Some(rate) = packets
                    .iter()
                    .map(|packet| packet.sdr_config.sample_rate)
                    .find(|rate| *rate > 0.0)
            {
                stream_sample_rate = Some(rate);
            }
            header_chunks_left -= 1;
            if stream_sample_rate.is_some() || header_chunks_left == 0 {
                // Drop the parser the moment it has served its purpose.
                // Two reasons, and the second is the important one: the
                // probe is not here to decode the samples it is timing,
                // and a parser fed something that is not this protocol
                // buffers what it cannot frame — over a multi-second probe
                // at 60 MB/s that is a lot of memory to hold for a number
                // we already have or are never going to get. A parse
                // failure is not a probe failure: bytes are what is being
                // measured and the rate is a bonus.
                parser = None;
            }
        }

        if meter.observe(Instant::now(), chunk.len()) {
            break;
        }
    }

    let measurement = meter.finish().filter(|_| meter.is_complete());
    match measurement {
        Some(m) => {
            let m = ThroughputMeasurement {
                stream_sample_rate,
                ..m
            };
            debug!("Link throughput probe of {url}: {m}");
            Ok(m)
        }
        None => Err(Error::Protocol(format!(
            "Link throughput probe of {url} did not complete: {} bytes arrived after the \
             {:.2} s settle window{}. Nothing was measured, which is not the same as \
             measuring a slow link — no span can be ruled in or out on this.",
            meter.counted_bytes,
            LINK_PROBE_SETTLE.as_secs_f64(),
            if stream_ended {
                ", and the server then closed the connection"
            } else {
                ""
            },
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::{IQ_CLOCK_HZ, iq_sample_rate_for_bandwidth};

    /// The arithmetic behind every measured figure in this module's
    /// docs, and in commit f322c60's message.
    #[test]
    fn required_byte_rate_is_four_bytes_a_sample() {
        // --span 10M -> 15.36 MS/s -> ~61 MB/s. Measured: zero gaps.
        assert_eq!(required_byte_rate(15_360_000.0), 61_440_000.0);
        // --span 20M -> 30.72 MS/s -> ~123 MB/s. Measured: ~5% lost.
        assert_eq!(required_byte_rate(30_720_000.0), 122_880_000.0);
        // Full span needs a wired path, and then some.
        assert_eq!(required_byte_rate(IQ_CLOCK_HZ), 245_760_000.0);

        // float32 doubles it; json has no fixed size per sample and so
        // has no computable floor (0.0 means "unknown", not "free").
        assert_eq!(
            required_byte_rate_for_format(15_360_000.0, StreamFormat::Float32),
            122_880_000.0
        );
        assert_eq!(
            required_byte_rate_for_format(15_360_000.0, StreamFormat::Float16),
            61_440_000.0
        );
        assert_eq!(
            required_byte_rate_for_format(15_360_000.0, StreamFormat::Json),
            0.0
        );
    }

    /// Nonsense in, zero out — never a negative or NaN budget that a
    /// comparison would silently pass.
    #[test]
    fn required_byte_rate_refuses_nonsense_rates() {
        assert_eq!(required_byte_rate(0.0), 0.0);
        assert_eq!(required_byte_rate(-1.0), 0.0);
        assert_eq!(required_byte_rate(f64::NAN), 0.0);
        assert_eq!(required_byte_rate(f64::INFINITY), 0.0);
    }

    /// The deployment ceiling that started all this: a gigabit link is
    /// 125 MB/s at best, and real ones deliver less.
    #[test]
    fn the_span_that_fits_gigabit_is_the_one_measured_to_work() {
        // 30.72 MS/s asks for 122.9 MB/s of a link whose theoretical
        // ceiling is 125 MB/s — no headroom at all, and the measured run
        // lost ~5%.
        assert!(required_byte_rate(30_720_000.0) > 0.98 * 125e6);
        // 15.36 MS/s asks for less than half of it, which is why that
        // capture came back contiguous.
        assert!(required_byte_rate(15_360_000.0) < 0.5 * 125e6);

        // A link measured at that ~5% shortfall is told to drop one rung
        // — to 12.288 MHz of span, which covers the --span 10M that was
        // measured working and does not cover the --span 20M that was
        // measured failing.
        let measured = 0.95 * 122_880_000.0;
        let advice = max_sustainable_span(measured);
        assert_eq!(advice, 12_288_000.0);
        assert!(advice >= 10e6, "--span 10M must still be offered");
        assert!(advice < 20e6, "--span 20M must not be offered");
    }

    /// The two link speeds this crate has actually measured with `curl`
    /// on `/stream`, resolved to spans.
    #[test]
    fn measured_links_resolve_to_ladder_rungs() {
        // WiFi 7 station to station, ~75 MB/s: 15.36 MS/s (61.4 MB/s)
        // fits, 30.72 (122.9) does not.
        assert_eq!(
            max_sustainable_sample_rate_for_format(75e6, DEFAULT_LINK_FORMAT),
            15_360_000.0
        );
        assert_eq!(max_sustainable_span(75e6), 12_288_000.0);

        // The original link, ~57 MB/s: not enough for 15.36 MS/s, so the
        // honest answer is a rung lower.
        assert_eq!(
            max_sustainable_sample_rate_for_format(57e6, DEFAULT_LINK_FORMAT),
            7_680_000.0
        );
        assert_eq!(max_sustainable_span(57e6), 6_144_000.0);
    }

    /// The inversion has to land on the ladder, not near it: every span
    /// it names must round-trip to a rung that genuinely fits, and the
    /// next rung up must genuinely not.
    #[test]
    fn max_sustainable_span_round_trips_through_the_ladder() {
        for measured in [
            0.5e6, 1e6, 5e6, 12e6, 30e6, 57e6, 61.44e6, 75e6, 122e6, 125e6, 250e6, 1e9,
        ] {
            let span = max_sustainable_span(measured);
            assert!(span > 0.0, "{measured} B/s should fit some rung");

            // The span names a rung, and that rung fits the measurement.
            let rate = iq_sample_rate_for_bandwidth(span);
            assert_eq!(
                rate,
                max_sustainable_sample_rate_for_format(measured, DEFAULT_LINK_FORMAT),
                "span {span} must round-trip to the rung it came from",
            );
            assert!(
                required_byte_rate(rate) <= measured,
                "{measured} B/s cannot carry the {rate} S/s rung it was offered",
            );

            // …and it is the *widest* such rung: anything faster does not
            // fit (unless we are already at the top of the ladder).
            let faster: Vec<f64> = iq_sample_rates()
                .into_iter()
                .filter(|r| *r > rate)
                .collect();
            for r in faster {
                assert!(
                    required_byte_rate(r) > measured,
                    "{r} S/s also fits {measured} B/s and is wider — the answer was not maximal",
                );
            }
        }
    }

    /// A link too narrow for the slowest rung gets no recommendation at
    /// all, rather than a span of zero dressed up as advice.
    #[test]
    fn max_sustainable_span_reports_nothing_when_no_rung_fits() {
        // The slowest rung is 120 kS/s = 480 kB/s.
        assert_eq!(max_sustainable_span(479_999.0), 0.0);
        assert_eq!(max_sustainable_span(480_000.0), 96_000.0);
        assert_eq!(max_sustainable_span(0.0), 0.0);
        assert_eq!(max_sustainable_span(-1.0), 0.0);
        assert_eq!(max_sustainable_span(f64::NAN), 0.0);
        // JSON has no computable per-sample size, so no rung can be
        // shown to fit.
        assert_eq!(
            max_sustainable_span_for_format(1e9, StreamFormat::Json),
            0.0
        );
    }

    /// **The lying-probe test.**
    ///
    /// The RTSA server hands over its pre-connect backlog at connect,
    /// faster than real time. A meter that counts those bytes measures
    /// the socket, not the link, and reports that a span which cannot
    /// fit is fine. Here the burst runs at ~200 MB/s and the steady
    /// stream at 10 MB/s; only the second figure is the link.
    #[test]
    fn the_connect_burst_does_not_inflate_the_measurement() {
        let t0 = Instant::now();
        let settle = Duration::from_millis(500);
        let window = Duration::from_secs(1);

        // 60 MB of backlog inside the first 300 ms.
        let burst = [
            (100u64, 20_000_000usize),
            (200, 20_000_000),
            (300, 20_000_000),
        ];
        // Then the real stream: 100 kB every 10 ms == 10 MB/s.
        let steady = (60..=200).map(|tick| (tick * 10, 100_000usize));

        let mut meter = ThroughputMeter::starting_at(t0, settle, window);
        for (ms, bytes) in burst.into_iter().chain(steady) {
            if meter.observe(t0 + Duration::from_millis(ms), bytes) {
                break;
            }
        }

        let m = meter.finish().expect("the window should have closed");
        assert!(meter.is_complete());
        assert!(
            (m.byte_rate - 10e6).abs() / 10e6 < 0.03,
            "measured {} B/s; the steady stream is 10 MB/s and the 200 MB/s connect burst \
             must not be in it",
            m.byte_rate,
        );
        // The burst is reported, not silently swallowed: 60 MB of
        // backlog plus the mark observation.
        assert!(m.settle_bytes >= 60_000_000);
        assert!(
            (m.window.as_secs_f64() - 1.0).abs() < 0.02,
            "window was {:?}",
            m.window
        );
        assert_eq!(m.stream_sample_rate, None, "no packet header was parsed");
    }

    /// The same trace with no settle window, to show the defect is real
    /// and that the constant is what prevents it. A probe like this
    /// would report a 60 MB/s path and wave through a span the link
    /// cannot carry.
    #[test]
    fn without_a_settle_window_the_same_trace_measures_six_times_too_fast() {
        let t0 = Instant::now();
        let mut meter = ThroughputMeter::starting_at(t0, Duration::ZERO, Duration::from_secs(1));
        let burst = [
            (100u64, 20_000_000usize),
            (200, 20_000_000),
            (300, 20_000_000),
        ];
        let steady = (60..=200).map(|tick| (tick * 10, 100_000usize));
        for (ms, bytes) in burst.into_iter().chain(steady) {
            if meter.observe(t0 + Duration::from_millis(ms), bytes) {
                break;
            }
        }
        let m = meter.finish().expect("the window should have closed");
        assert!(
            m.byte_rate > 40e6,
            "expected the backlog to inflate this measurement, got {} B/s",
            m.byte_rate
        );
    }

    /// A stream that delivers nothing after settle has no measurement,
    /// and must not be reported as 0 MB/s — that would condemn every
    /// span on the ladder on the strength of a failed probe.
    #[test]
    fn a_stream_with_nothing_after_settle_has_no_measurement() {
        let t0 = Instant::now();
        let mut meter =
            ThroughputMeter::starting_at(t0, Duration::from_millis(500), Duration::from_secs(1));
        meter.observe(t0 + Duration::from_millis(100), 1_000_000);
        meter.observe(t0 + Duration::from_millis(400), 1_000_000);
        assert!(!meter.is_complete());
        assert!(meter.finish().is_none());
    }

    /// One observation past the settle window spans no interval, so
    /// there is no rate to compute — not an infinite one.
    #[test]
    fn a_single_observation_after_settle_has_no_measurement() {
        let t0 = Instant::now();
        let mut meter =
            ThroughputMeter::starting_at(t0, Duration::from_millis(500), Duration::from_secs(1));
        meter.observe(t0 + Duration::from_millis(600), 10_000_000);
        assert!(meter.finish().is_none());
    }

    /// Once complete the meter is frozen: a late chunk arriving while
    /// the caller tears the connection down cannot move the answer.
    #[test]
    fn a_complete_meter_ignores_later_observations() {
        let t0 = Instant::now();
        let mut meter =
            ThroughputMeter::starting_at(t0, Duration::ZERO, Duration::from_millis(100));
        meter.observe(t0, 0);
        assert!(meter.observe(t0 + Duration::from_millis(100), 1_000_000));
        let before = meter.finish().expect("complete");
        assert!(meter.observe(t0 + Duration::from_millis(200), 999_000_000));
        assert_eq!(meter.finish().expect("still complete"), before);
    }

    /// The display string is what an operator reads in a warning, so it
    /// has to carry the evidence and not just the verdict.
    #[test]
    fn display_reports_window_and_settle_alongside_the_rate() {
        let m = ThroughputMeasurement {
            byte_rate: 57_300_000.0,
            bytes: 114_600_000,
            window: Duration::from_secs(2),
            settle: Duration::from_millis(500),
            settle_bytes: 21_400_000,
            stream_sample_rate: Some(15_360_000.0),
        };
        let s = m.to_string();
        assert!(s.contains("57.3 MB/s"), "{s}");
        assert!(s.contains("2.00 s"), "{s}");
        assert!(s.contains("0.50 s"), "{s}");
        assert!(s.contains("21.4 MB"), "{s}");
        assert!((m.megabytes_per_second() - 57.3).abs() < 1e-9);
        // 57.3 MB/s is short of the 61.44 the stream needed, and the
        // advice reflects it.
        assert_eq!(m.max_sustainable_span_hz(), 6_144_000.0);
    }

    /// Stream from a socket the way the RTSA server does: a backlog
    /// dumped at connect as fast as the socket takes it, then a steady
    /// rate.
    ///
    /// Returns the base URL. The listener serves exactly one connection
    /// and stops writing after `total`, so a probe that never finishes
    /// hits its stall timeout rather than hanging the suite.
    async fn spawn_bursty_stream_server(
        backlog_bytes: usize,
        steady_bytes_per_tick: usize,
        tick: Duration,
        total: Duration,
    ) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            // Read past the request head; the probe sends no body.
            let mut scratch = [0u8; 4096];
            let _ = socket.read(&mut scratch).await;
            if socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                      Connection: close\r\n\r\n",
                )
                .await
                .is_err()
            {
                return;
            }
            // The connect backlog, written as fast as it is accepted.
            let backlog = vec![0u8; backlog_bytes];
            if socket.write_all(&backlog).await.is_err() {
                return;
            }
            // Then the device's real rate.
            let block = vec![0u8; steady_bytes_per_tick];
            let deadline = Instant::now() + total;
            while Instant::now() < deadline {
                if socket.write_all(&block).await.is_err() {
                    return;
                }
                tokio::time::sleep(tick).await;
            }
        });
        format!("http://{addr}")
    }

    /// End to end over a real socket: the connect backlog must not reach
    /// the answer.
    ///
    /// The unit tests above pin the settle logic against synthetic
    /// instants; this one pins that the probe wires it up — that the
    /// settle window is anchored at the response and that what is counted
    /// is the steady stream. The server dumps 40 MB at connect (which on
    /// loopback lands in milliseconds, so counting it would read hundreds
    /// of MB/s) and then holds 20 MB/s.
    #[tokio::test]
    async fn the_probe_measures_the_steady_rate_not_the_connect_backlog() {
        let url = spawn_bursty_stream_server(
            40_000_000,
            200_000,
            Duration::from_millis(10),
            Duration::from_millis(1_500),
        )
        .await;

        let m = measure_link_throughput(&url, Duration::from_millis(300))
            .await
            .expect("the probe should complete against a stream that keeps sending");

        assert!(
            m.byte_rate < 40e6,
            "measured {} B/s — the 40 MB connect backlog is in the answer",
            m.byte_rate
        );
        assert!(
            m.byte_rate > 5e6,
            "measured {} B/s — the steady ~20 MB/s stream is missing from the answer",
            m.byte_rate
        );
        assert!(
            m.settle_bytes >= 40_000_000,
            "the backlog should be reported as discarded, got {}",
            m.settle_bytes
        );
        assert_eq!(m.settle, LINK_PROBE_SETTLE);
        assert!(m.window >= Duration::from_millis(280), "{:?}", m.window);
        // Nothing in that stream is an RTSA packet, so no rate is
        // claimed — and the byte measurement stands regardless.
        assert_eq!(m.stream_sample_rate, None);
    }

    /// A server that hangs up before the window closes is an error, not
    /// a slow link. The distinction matters: "0 MB/s" would condemn every
    /// span on the ladder.
    #[tokio::test]
    async fn a_stream_that_ends_early_is_an_error_not_a_rate() {
        let url = spawn_bursty_stream_server(
            1_000_000,
            100_000,
            Duration::from_millis(10),
            Duration::from_millis(600),
        )
        .await;

        let err = measure_link_throughput(&url, Duration::from_secs(2))
            .await
            .expect_err("a stream that stops mid-window has not been measured");
        let msg = err.to_string();
        assert!(msg.contains("did not complete"), "got: {msg}");
    }

    /// The probe must reject a URL it should never fetch before it opens
    /// a socket, and must not answer "0 MB/s" for an unreachable server.
    #[tokio::test]
    async fn a_probe_of_a_bad_url_is_an_error_not_a_rate() {
        let err = measure_link_throughput("file:///etc/passwd", Duration::from_millis(10))
            .await
            .expect_err("non-HTTP schemes must be refused");
        assert!(err.to_string().contains("HTTP/HTTPS"), "got: {err}");

        let err = measure_link_throughput("not a url", Duration::from_millis(10))
            .await
            .expect_err("an unparseable URL must be refused");
        assert!(err.to_string().contains("Invalid base URL"), "got: {err}");
    }
}
