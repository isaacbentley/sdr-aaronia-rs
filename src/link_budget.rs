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
//! Every helper answers with an `Option`: `None` means "no budget can be
//! computed" (a JSON stream has no fixed size per sample, a nonsense
//! rate has no requirement, a narrow enough link fits no rung) — never a
//! `0.0` that a comparison would silently wave through.
//!
//! ```
//! use sdr_aaronia_rs::link_budget::{max_sustainable_span, required_byte_rate};
//!
//! // 15.36 MS/s at 4 bytes a sample.
//! assert_eq!(required_byte_rate(15_360_000.0), Some(61_440_000.0));
//!
//! // A path measured at 75 MB/s (WiFi 7, station to station) carries the
//! // 15.36 MS/s rung and nothing above it: 12.288 MHz of usable span.
//! assert_eq!(max_sustainable_span(75_000_000.0), Some(12_288_000.0));
//! ```
//!
//! The end-to-end probe is [`measure_link_throughput`]. Deliberately
//! *end to end*: the bottleneck may be the server, a switch, the air, or
//! this host, and only bytes counted off the socket see all of them. The
//! NIC's advertised link speed sees none of them.

use std::time::{Duration, Instant};

use crate::http_endpoints::{
    AuthMethod, HttpEndpointsClient, StreamParams, StreamParamsBuilder, rtsa_client_builder,
    validate_base_url,
};
use crate::http_streaming::{PayloadType, StreamFormat};
use crate::utils::{iq_sample_rates, usable_bandwidth_hz};
use crate::{Error, Result};

use tracing::{debug, info};

/// The wire format the byte-rate helpers assume when none is named:
/// `int16`, 4 bytes a sample.
///
/// This is [`StreamFormat::CAPTURE_DEFAULT`] — the same constant
/// [`crate::HttpSourceBuilder`] streams by default — so it is the format
/// whose arithmetic an operator is actually asking about, and the three
/// cannot drift apart. Use the `_for_format` variants for anything else:
/// `float32` is 8 bytes a sample and needs twice the link.
pub const DEFAULT_LINK_FORMAT: StreamFormat = StreamFormat::CAPTURE_DEFAULT;

/// How much of a freshly-opened stream to throw away before counting,
/// when the caller does not choose otherwise.
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
///
/// The backlog is the *server's* buffering policy, not a protocol
/// constant: a different RTSA version or a deeper history buffer can
/// hand over more. This is only the default —
/// [`measure_link_throughput_with`] takes the settle window as a
/// parameter for paths measured to need a longer one.
pub const LINK_PROBE_SETTLE: Duration = Duration::from_millis(500);

/// Shortest counting window [`measure_link_throughput_with`] accepts.
///
/// Below this the "measurement" is two adjacent socket reads: the kernel
/// hands over whatever its buffers hold in microseconds, and the
/// computed rate reflects that buffering rather than the link — a probe
/// asked for a zero-length window would happily report gigabytes a
/// second over a link that cannot sustain one. A tenth of a second is
/// still far too short for a *good* measurement (use seconds); it is
/// merely where the answer stops being about the kernel.
pub const MIN_PROBE_WINDOW: Duration = Duration::from_millis(100);

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

/// How much of the stream the probe scans for a packet header before
/// giving up on learning the device's rate.
///
/// The rate lives in each packet's JSON header, so the scan reads
/// headers only and never decodes a payload. A megabyte is several
/// packets into a genuine RTSA stream (server chunks are ~157 KiB) and a
/// hard stop for a stream that turns out not to be this protocol at
/// all. Giving up is logged rather than silent, but it is not a probe
/// failure: bytes are what is being measured, and the rate is only the
/// yardstick for interpreting them.
const PROBE_HEADER_SCAN_BYTES: usize = 1 << 20;

/// Bytes a second the path must sustain to carry `sample_rate_hz` in
/// `format`, or `None` when no budget can be computed — a rate that is
/// not a positive finite number, or [`StreamFormat::Json`], whose
/// samples have no fixed size.
///
/// The payload floor, not the wire total: each packet also carries a
/// JSON metadata header. A comparison against *raw* wire bytes therefore
/// cannot fire on protocol overhead; a comparison against delivered IQ
/// payload (what `HttpSource`'s passive check counts) is exact.
pub fn required_byte_rate_for_format(sample_rate_hz: f64, format: StreamFormat) -> Option<f64> {
    if !sample_rate_hz.is_finite() || sample_rate_hz <= 0.0 {
        return None;
    }
    let bytes_per_sample = format.iq_bytes_per_sample()? as f64;
    Some(sample_rate_hz * bytes_per_sample)
}

/// Bytes a second the path must sustain to carry `sample_rate_hz` in the
/// [`DEFAULT_LINK_FORMAT`] (4 bytes a sample).
pub fn required_byte_rate(sample_rate_hz: f64) -> Option<f64> {
    required_byte_rate_for_format(sample_rate_hz, DEFAULT_LINK_FORMAT)
}

/// The fastest rung of the default-clock decimation ladder whose stream
/// fits `measured_byte_rate_hz`, in samples a second.
///
/// Inverts [`required_byte_rate_for_format`] through
/// [`crate::utils::iq_sample_rates`] rather than solving for a rate and
/// rounding: the device has no rates between the rungs, so the only
/// useful answer is one the hardware can actually be set to.
///
/// **This is the ECO / default-clock ladder.** A full V6 on a faster
/// receiver clock has rungs this ladder does not; when the device's own
/// rate is known, prefer [`max_sustainable_sample_rate_below`], whose
/// answers are anchored to that rate and so exist on the device's actual
/// ladder whatever its clock.
///
/// Returns `None` when even the slowest rung (120 kS/s, 480 kB/s) does
/// not fit — a link that narrow is the problem, not the span — or when
/// the inputs admit no computation at all.
pub fn max_sustainable_sample_rate_for_format(
    measured_byte_rate_hz: f64,
    format: StreamFormat,
) -> Option<f64> {
    if !measured_byte_rate_hz.is_finite() || measured_byte_rate_hz <= 0.0 {
        return None;
    }
    iq_sample_rates()
        .into_iter()
        .filter(|rate| rung_fits(*rate, format, measured_byte_rate_hz))
        // The ladder is ordered fastest-first, so the first match is the
        // answer; reducing over `max` rather than taking `next()` keeps
        // this correct if that order ever changes.
        .reduce(f64::max)
}

/// Whether the `rate_hz` rung's stream fits `measured_byte_rate_hz` in
/// `format` — the one predicate behind every "widest rung" search here.
fn rung_fits(rate_hz: f64, format: StreamFormat, measured_byte_rate_hz: f64) -> bool {
    required_byte_rate_for_format(rate_hz, format)
        .is_some_and(|needed| needed <= measured_byte_rate_hz)
}

/// The decimation ladder whose top rung is `device_rate_hz` — the
/// device's own reported rate sits on its ladder by definition.
pub(crate) fn device_ladder(device_rate_hz: f64) -> [f64; 10] {
    crate::utils::iq_ladder_from_top(device_rate_hz)
}

/// The fastest rate at or below `device_rate_hz`, reachable by halving
/// it, whose stream fits `measured_byte_rate_hz` in `format`.
///
/// The ladder helpers above invert through the **default-clock** ladder,
/// and a full V6 on a faster receiver clock has rungs that ladder does
/// not (see [`crate::utils::iq_sample_rates_for_clock`] and its caveat).
/// Every RTSA ladder halves rung to rung, though, so rates reached by
/// halving the rate the device itself reported are on its ladder
/// whatever the clock — this is what `HttpSource`'s passive check uses
/// for its remedy, so the span it names is always one the device can be
/// set to.
///
/// Searches as deep as a ladder goes (nine halvings, `1/512`); `None`
/// when even that does not fit, which points at the link rather than the
/// span.
pub fn max_sustainable_sample_rate_below(
    device_rate_hz: f64,
    measured_byte_rate_hz: f64,
    format: StreamFormat,
) -> Option<f64> {
    if !device_rate_hz.is_finite()
        || device_rate_hz <= 0.0
        || !measured_byte_rate_hz.is_finite()
        || measured_byte_rate_hz <= 0.0
    {
        return None;
    }
    device_ladder(device_rate_hz)
        .into_iter()
        .filter(|rate| rung_fits(*rate, format, measured_byte_rate_hz))
        // As above: reduce over `max` rather than trusting the ladder's
        // fastest-first order.
        .reduce(f64::max)
}

/// The widest span on the default-clock decimation ladder that fits
/// `measured_byte_rate_hz`, in Hz, for the [`DEFAULT_LINK_FORMAT`].
///
/// "Span" here is the usable (alias-free) bandwidth the rung delivers —
/// the same quantity `--span` selects, so the answer can be handed
/// straight back to the operator. Round-trips: feeding it to
/// [`crate::utils::iq_sample_rate_for_bandwidth`] returns the rung it
/// came from.
///
/// Returns `None` when nothing on the ladder fits.
pub fn max_sustainable_span(measured_byte_rate_hz: f64) -> Option<f64> {
    max_sustainable_span_for_format(measured_byte_rate_hz, DEFAULT_LINK_FORMAT)
}

/// The widest span on the default-clock decimation ladder that fits
/// `measured_byte_rate_hz`, in Hz, for `format`.
pub fn max_sustainable_span_for_format(
    measured_byte_rate_hz: f64,
    format: StreamFormat,
) -> Option<f64> {
    Some(usable_bandwidth_hz(max_sustainable_sample_rate_for_format(
        measured_byte_rate_hz,
        format,
    )?))
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
    /// Interval actually discarded before counting began: at least the
    /// configured settle window (see [`LINK_PROBE_SETTLE`] for why one
    /// exists), extended to whenever the first post-settle observation —
    /// the mark — really arrived. Reporting the configured figure here
    /// would misstate the backlog's rate whenever the mark came late.
    pub settle: Duration,
    /// Bytes that arrived before the mark and were discarded. Reported
    /// rather than hidden: at a wide span this is the pre-connect
    /// backlog, and its size (over [`Self::settle`]) is itself
    /// diagnostic.
    pub settle_bytes: u64,
    /// The rate the device was streaming at while this was measured,
    /// read from an IQ packet header — `None` when no IQ header
    /// reporting a positive rate was found. IQ only: a spectra or
    /// histogram header without `sampleFrequency` yields a *frame* rate
    /// orders of magnitude below the IQ rate, which is no yardstick.
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

    /// The widest span this measurement proves the path can carry, in Hz,
    /// for a capture in `format` — see [`max_sustainable_span_for_format`],
    /// and note "proves": if the device was streaming narrower than the
    /// link could carry, this is a floor on the answer rather than the
    /// answer. `None` when nothing on the ladder fits.
    ///
    /// The format is a parameter because a measurement does not know what
    /// it was probed in, and the answer doubles or halves with the bytes
    /// per sample: a 75 MB/s path carries the 15.36 MS/s rung in `int16`
    /// but only 7.68 MS/s in `float32`. Pass the format the capture will
    /// use — normally the same one handed to
    /// [`measure_link_throughput_with`].
    pub fn max_sustainable_span_hz(&self, format: StreamFormat) -> Option<f64> {
        max_sustainable_span_for_format(self.byte_rate, format)
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

/// The passive link-budget check's verdict, as data.
///
/// `HttpSource` computes this once per stream configuration and
/// publishes it on the shared `StreamStats` handle, so a programmatic
/// consumer — a GUI, an orchestrator auto-narrowing the span, the
/// bindings — reads the figures the check computed instead of scraping
/// them out of a log line. The stream-gap warning restates them from
/// here too, so its cross-reference cites the actual numbers rather
/// than pointing at a log line possibly hours up the scroll.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkBudgetVerdict {
    /// Device-reported sample rate the check compared against, in Hz.
    pub sample_rate: f64,
    /// Wire format the requirement was computed for.
    pub format: StreamFormat,
    /// Bytes one IQ sample occupies in [`Self::format`] — denormalized
    /// from the format (which always has an answer here: [`Self::judge`]
    /// refuses formats that do not) so report text can cite it without
    /// re-deriving.
    pub bytes_per_sample: usize,
    /// Bytes a second `sample_rate` requires in `format`.
    pub required_byte_rate: f64,
    /// What the stream delivered, counted as IQ payload bytes.
    pub measured: ThroughputMeasurement,
    /// Whether delivery fell short of the requirement beyond measurement
    /// tolerance.
    pub short: bool,
    /// On a shortfall, the fastest rate at or below [`Self::sample_rate`]
    /// whose stream fits the measurement — halved down from the device's
    /// own rate via [`max_sustainable_sample_rate_below`], so it exists
    /// on its ladder. `None` when the path is not short, or when nothing
    /// fits.
    pub fit_sample_rate_hz: Option<f64>,
    /// The usable span [`Self::fit_sample_rate_hz`] delivers, in Hz.
    pub fit_span_hz: Option<f64>,
}

impl LinkBudgetVerdict {
    /// Judge a measurement against the rate the device reported.
    ///
    /// The one place the verdict arithmetic lives: shortness is measured
    /// against `required * tolerance` (tolerance just under 1.0 absorbs
    /// measurement quantisation, not link slack), and the remedy rung is
    /// halved down from the device's own rate so it exists on its ladder.
    /// `HttpSource`'s passive check routes through here, and a caller of
    /// [`measure_link_throughput_with`] can too, instead of re-deriving
    /// shortness and remedy by hand: the fit fields are populated only on
    /// a shortfall, and the span matches the fit rate, by construction.
    ///
    /// `None` when no requirement can be computed — a format with no
    /// fixed bytes per sample, or a rate that is not a positive finite
    /// number — since judging against nothing would be the "failed
    /// measurement becomes a verdict" mistake this module exists to
    /// prevent.
    pub fn judge(
        sample_rate_hz: f64,
        format: StreamFormat,
        measured: ThroughputMeasurement,
        tolerance: f64,
    ) -> Option<Self> {
        let bytes_per_sample = format.iq_bytes_per_sample()?;
        let required = required_byte_rate_for_format(sample_rate_hz, format)?;
        let short = measured.byte_rate < required * tolerance;
        let fit_sample_rate_hz = if short {
            max_sustainable_sample_rate_below(sample_rate_hz, measured.byte_rate, format)
        } else {
            None
        };
        Some(Self {
            sample_rate: sample_rate_hz,
            format,
            bytes_per_sample,
            required_byte_rate: required,
            measured,
            short,
            fit_sample_rate_hz,
            fit_span_hz: fit_sample_rate_hz.map(usable_bandwidth_hz),
        })
    }
}

/// Counts bytes against a wall clock, discarding an initial settle
/// window.
///
/// Split out from [`measure_link_throughput`] for two reasons: the live
/// `HttpSource` runs the same check passively on the stream it is
/// already reading rather than opening a second connection, and the
/// settle logic — the part a naive implementation gets wrong — is
/// testable here against synthetic timestamps instead of a real socket.
#[derive(Debug, Clone)]
pub struct ThroughputMeter {
    settle: Duration,
    window: Duration,
    opened: Instant,
    /// First observation after the settle period. Counting runs from
    /// here, and this observation's own bytes are *not* counted — see
    /// [`Self::observe`].
    mark: Option<Instant>,
    /// Latest observation that was counted.
    last: Option<Instant>,
    discarded_bytes: u64,
    counted_bytes: u64,
    /// Set by the observation at or past `mark + window`, which is itself
    /// excluded from the count (see [`Self::observe`]).
    closed: bool,
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
            closed: false,
        }
    }

    /// Record `bytes` received at `at`. Returns whether the measurement
    /// is complete.
    ///
    /// Bytes before `opened + settle` are discarded, for the reason
    /// [`LINK_PROBE_SETTLE`] documents. So are the bytes of the first
    /// observation past that boundary: they accumulated *before* it, and
    /// counting them against a window that starts at that instant is the
    /// same inflation in miniature.
    ///
    /// The observation that crosses `mark + window` closes the window
    /// and is excluded too, in both directions — its bytes are not
    /// counted and its instant does not extend the interval. Counting it
    /// would average over however long the caller went without
    /// observing: one multi-second stall between sweeps would otherwise
    /// dilute a healthy stream's figure into a spurious shortfall. What
    /// is counted is exactly the bytes that arrived between the mark and
    /// the last counted observation, over exactly that interval.
    pub fn observe(&mut self, at: Instant, bytes: usize) -> bool {
        if self.closed {
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
                if at.saturating_duration_since(mark) >= self.window {
                    self.closed = true;
                } else {
                    self.counted_bytes = self.counted_bytes.saturating_add(bytes);
                    self.last = Some(at);
                }
            }
        }
        self.closed
    }

    /// Whether the counting window has closed.
    pub fn is_complete(&self) -> bool {
        self.closed
    }

    /// The measurement, or `None` if there is not enough of one.
    ///
    /// `None` until the window has closed — a partial count is not a
    /// measurement, and the check is enforced here rather than left as a
    /// convention every caller must remember to pair with
    /// [`Self::is_complete`]. Also `None` when no bytes were counted
    /// (zero-byte observations still advance the clock, and "the stream
    /// delivered nothing" is a failed measurement, not a rate of zero),
    /// and `None` when the counted interval covers less than half the
    /// configured window: excluding the boundary-crossing observation
    /// means a burst-then-stall stream can close the window having
    /// observed only a few milliseconds of kernel-buffer drain, and a
    /// rate computed from that would be the socket-buffer artifact
    /// [`MIN_PROBE_WINDOW`] refuses at the front door. A caller must not
    /// turn any of these into "0 MB/s" — see the constraint at the top
    /// of [`measure_link_throughput`].
    pub fn finish(&self) -> Option<ThroughputMeasurement> {
        if !self.closed || self.counted_bytes == 0 {
            return None;
        }
        let mark = self.mark?;
        let last = self.last?;
        let window = last.saturating_duration_since(mark);
        if window.is_zero() || window < self.window / 2 {
            return None;
        }
        Some(ThroughputMeasurement {
            byte_rate: self.counted_bytes as f64 / window.as_secs_f64(),
            bytes: self.counted_bytes,
            window,
            settle: mark.saturating_duration_since(self.opened),
            settle_bytes: self.discarded_bytes,
            stream_sample_rate: None,
        })
    }
}

/// Incremental header sniff: feed stream chunks, learn the rate the
/// device is streaming at.
///
/// Framing lives in [`crate::http_streaming::scan_packet_header`] — the
/// stream parser's own header scan — so this holds only what is probe
/// policy: read IQ headers only (a spectra/histogram header without
/// `sampleFrequency` derives a *frame* rate orders of magnitude below
/// the IQ rate — the same nonsense yardstick `HttpSource`'s passive
/// check refuses — and on a mixed mission such a header can come
/// first), skip zero-rate headers (status packets with
/// `startTime == endTime` and no `sampleFrequency` must not read as "not
/// an RTSA stream"), spend the [`PROBE_HEADER_SCAN_BYTES`] budget, and
/// keep no more buffered than a possibly-unfinished header — bytes are
/// dropped as they are ruled out, so the buffer holds at most one
/// candidate still awaiting its terminator (re-scanned as chunks extend
/// it, bounded by the budget) rather than the whole accumulation.
struct RateSniffer {
    /// Bytes not yet ruled out as (the start of) a header.
    buf: Vec<u8>,
    /// Total bytes ever fed, for the give-up budget.
    fed: usize,
}

impl RateSniffer {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            fed: 0,
        }
    }

    /// Feed one chunk; `Some(rate)` once a header reports a positive
    /// sample rate.
    fn feed(&mut self, chunk: &[u8]) -> Option<f64> {
        self.buf.extend_from_slice(chunk);
        self.fed += chunk.len();
        loop {
            match crate::http_streaming::scan_packet_header(&self.buf) {
                crate::http_streaming::HeaderScan::Header {
                    metadata,
                    payload_start,
                    ..
                } => {
                    self.buf.drain(..payload_start);
                    // Only an IQ header carries the rate this is a
                    // yardstick for. A spectra/histogram header derives a
                    // frame rate instead, and taking the first positive
                    // one on a mixed mission would make a starved link
                    // look loaded against a requirement of a few kB/s.
                    if metadata.payload != PayloadType::Iq {
                        continue;
                    }
                    let rate = metadata.sample_rate();
                    if rate > 0.0 {
                        return Some(rate);
                    }
                    // A zero-rate IQ header: keep scanning what remains.
                }
                crate::http_streaming::HeaderScan::Incomplete { skippable, .. } => {
                    self.buf.drain(..skippable);
                    return None;
                }
            }
        }
    }

    /// Whether the scan budget is spent with no rate found.
    fn budget_spent(&self) -> bool {
        self.fed > PROBE_HEADER_SCAN_BYTES
    }
}

/// Measure what the path to `base_url` actually delivers, by streaming
/// from it and counting bytes off the socket for `duration`.
///
/// The total cost is [`LINK_PROBE_SETTLE`] + `duration` plus connect
/// time; the settle period is discarded before counting starts, because
/// the server's connect backlog arrives faster than real time and would
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
/// A `duration` under [`MIN_PROBE_WINDOW`], an unreachable server, a
/// non-success status, a stream that stops sending, or a stream that
/// ends before the window closes. A failed measurement is always an
/// error and never a rate — "the server was unreachable" and "the link
/// delivers 0 MB/s" must not be confusable, since the second one
/// condemns every span on the ladder.
pub async fn measure_link_throughput(
    base_url: &str,
    duration: Duration,
) -> Result<ThroughputMeasurement> {
    let params = StreamParamsBuilder::new()
        .format(DEFAULT_LINK_FORMAT)
        .build();
    measure_link_throughput_with(
        base_url,
        duration,
        AuthMethod::None,
        &params,
        LINK_PROBE_SETTLE,
    )
    .await
}

/// As [`measure_link_throughput`], with the authentication method, the
/// stream parameters, and the settle window named.
///
/// Pass the *same* [`StreamParams`] the capture will use. The format
/// matters to the arithmetic (`float32` is 8 bytes a sample, so an
/// `int16` measurement is half the story for a `float32` capture), and
/// the rest of the parameters matter to what is measured: a probe that
/// omitted the capture's `input` would measure the server's default
/// input, and one that omitted its `rate_reduction` would measure a
/// stream several times heavier than the one the capture opens.
///
/// `settle` is how much of the freshly-opened stream to discard before
/// counting — [`LINK_PROBE_SETTLE`] unless this server's connect backlog
/// is measured to need more. Passing a shorter one reintroduces the
/// inflation that constant documents; that is the caller's choice to
/// make, not this function's to prevent.
pub async fn measure_link_throughput_with(
    base_url: &str,
    duration: Duration,
    auth: AuthMethod,
    params: &StreamParams,
    settle: Duration,
) -> Result<ThroughputMeasurement> {
    if duration < MIN_PROBE_WINDOW {
        return Err(Error::Config(format!(
            "Link probe window {:?} is below the {:?} minimum: over so short an interval \
             the kernel's socket buffers set the answer, not the link, and the \"rate\" \
             can read as gigabytes a second over a path that cannot sustain one.",
            duration, MIN_PROBE_WINDOW,
        )));
    }

    // Same URL validation the source applies at construction — one shared
    // implementation, so the probe and the capture accept the same URLs.
    validate_base_url(base_url)?;

    // Same client settings as every other RTSA connection in this crate,
    // via the one shared builder.
    let client = rtsa_client_builder(Duration::from_secs(5)).build()?;

    params.validate()?;
    let url = params.stream_url(base_url);

    info!(
        "Measuring link throughput from {url}: discarding {:.2} s of connect backlog, \
         then counting for {:.2} s",
        settle.as_secs_f64(),
        duration.as_secs_f64(),
    );

    let response = auth.apply_to(client.get(&url)).send().await?;
    let mut response =
        HttpEndpointsClient::ensure_success(&format!("Link throughput probe of {url}"), response)?;

    let mut meter = ThroughputMeter::new(settle, duration);
    // Header sniff: learns the device's rate from packet headers, then
    // is dropped. `None` once the rate is known or the scan budget is
    // spent, so the steady-state loop does nothing but count.
    let mut sniffer: Option<RateSniffer> = Some(RateSniffer::new());
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

        if let Some(s) = sniffer.as_mut() {
            let found = s.feed(&chunk);
            let budget_spent = s.budget_spent();
            if found.is_some() {
                stream_sample_rate = found;
                sniffer = None;
            } else if budget_spent {
                debug!(
                    "No RTSA packet header with a positive sample rate in the first \
                     {PROBE_HEADER_SCAN_BYTES} bytes from {url}; measuring bytes only \
                     (stream_sample_rate will be None)"
                );
                sniffer = None;
            }
        }

        if meter.observe(Instant::now(), chunk.len()) {
            break;
        }
    }

    match meter.finish() {
        Some(m) => {
            let m = ThroughputMeasurement {
                stream_sample_rate,
                ..m
            };
            debug!("Link throughput probe of {url}: {m}");
            Ok(m)
        }
        // Two distinct failures, told apart so the operator is not sent
        // looking for a hang-up that never happened: a window that
        // closed but was observed too thinly (or counted nothing) is a
        // burst-then-stall, not an incomplete probe.
        None if meter.is_complete() => Err(Error::Protocol(format!(
            "Link throughput probe of {url} closed its {:.2} s window without a usable \
             measurement: only {} bytes were counted, over less than half the window — a \
             burst and then silence until a late chunk closed it, not a sustained rate. \
             Nothing was measured, which is not the same as measuring a slow link — no span \
             can be ruled in or out on this.",
            duration.as_secs_f64(),
            meter.counted_bytes,
        ))),
        None => Err(Error::Protocol(format!(
            "Link throughput probe of {url} did not complete: {} bytes arrived after the \
             {:.2} s settle window{}. Nothing was measured, which is not the same as \
             measuring a slow link — no span can be ruled in or out on this.",
            meter.counted_bytes,
            settle.as_secs_f64(),
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
    use crate::http_streaming::{ASCII_LINE_FEED, ASCII_RECORD_SEPARATOR};
    use crate::utils::{IQ_CLOCK_HZ, iq_sample_rate_for_bandwidth};

    /// The arithmetic behind every measured figure in this module's
    /// docs, and in commit f322c60's message.
    #[test]
    fn required_byte_rate_is_four_bytes_a_sample() {
        // --span 10M -> 15.36 MS/s -> ~61 MB/s. Measured: zero gaps.
        assert_eq!(required_byte_rate(15_360_000.0), Some(61_440_000.0));
        // --span 20M -> 30.72 MS/s -> ~123 MB/s. Measured: ~5% lost.
        assert_eq!(required_byte_rate(30_720_000.0), Some(122_880_000.0));
        // Full span needs a wired path, and then some.
        assert_eq!(required_byte_rate(IQ_CLOCK_HZ), Some(245_760_000.0));

        // float32 doubles it; json has no fixed size per sample and so
        // has no computable floor — `None`, which no comparison can
        // silently wave through.
        assert_eq!(
            required_byte_rate_for_format(15_360_000.0, StreamFormat::Float32),
            Some(122_880_000.0)
        );
        assert_eq!(
            required_byte_rate_for_format(15_360_000.0, StreamFormat::Float16),
            Some(61_440_000.0)
        );
        assert_eq!(
            required_byte_rate_for_format(15_360_000.0, StreamFormat::Json),
            None
        );
    }

    /// Nonsense in, `None` out — never a zero, negative or NaN budget
    /// that a comparison would silently pass.
    #[test]
    fn required_byte_rate_refuses_nonsense_rates() {
        assert_eq!(required_byte_rate(0.0), None);
        assert_eq!(required_byte_rate(-1.0), None);
        assert_eq!(required_byte_rate(f64::NAN), None);
        assert_eq!(required_byte_rate(f64::INFINITY), None);
    }

    /// The deployment ceiling that started all this: a gigabit link is
    /// 125 MB/s at best, and real ones deliver less.
    #[test]
    fn the_span_that_fits_gigabit_is_the_one_measured_to_work() {
        // 30.72 MS/s asks for 122.9 MB/s of a link whose theoretical
        // ceiling is 125 MB/s — no headroom at all, and the measured run
        // lost ~5%.
        assert!(required_byte_rate(30_720_000.0).unwrap() > 0.98 * 125e6);
        // 15.36 MS/s asks for less than half of it, which is why that
        // capture came back contiguous.
        assert!(required_byte_rate(15_360_000.0).unwrap() < 0.5 * 125e6);

        // A link measured at that ~5% shortfall is told to drop one rung
        // — to 12.288 MHz of span, which covers the --span 10M that was
        // measured working and does not cover the --span 20M that was
        // measured failing.
        let measured = 0.95 * 122_880_000.0;
        let advice = max_sustainable_span(measured).expect("some rung fits");
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
            Some(15_360_000.0)
        );
        assert_eq!(max_sustainable_span(75e6), Some(12_288_000.0));

        // The original link, ~57 MB/s: not enough for 15.36 MS/s, so the
        // honest answer is a rung lower.
        assert_eq!(
            max_sustainable_sample_rate_for_format(57e6, DEFAULT_LINK_FORMAT),
            Some(7_680_000.0)
        );
        assert_eq!(max_sustainable_span(57e6), Some(6_144_000.0));
    }

    /// The inversion has to land on the ladder, not near it: every span
    /// it names must round-trip to a rung that genuinely fits, and the
    /// next rung up must genuinely not.
    #[test]
    fn max_sustainable_span_round_trips_through_the_ladder() {
        for measured in [
            0.5e6, 1e6, 5e6, 12e6, 30e6, 57e6, 61.44e6, 75e6, 122e6, 125e6, 250e6, 1e9,
        ] {
            let span = max_sustainable_span(measured)
                .unwrap_or_else(|| panic!("{measured} B/s should fit some rung"));

            // The span names a rung, and that rung fits the measurement.
            let rate = iq_sample_rate_for_bandwidth(span);
            assert_eq!(
                Some(rate),
                max_sustainable_sample_rate_for_format(measured, DEFAULT_LINK_FORMAT),
                "span {span} must round-trip to the rung it came from",
            );
            assert!(
                required_byte_rate(rate).unwrap() <= measured,
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
                    required_byte_rate(r).unwrap() > measured,
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
        assert_eq!(max_sustainable_span(479_999.0), None);
        assert_eq!(max_sustainable_span(480_000.0), Some(96_000.0));
        assert_eq!(max_sustainable_span(0.0), None);
        assert_eq!(max_sustainable_span(-1.0), None);
        assert_eq!(max_sustainable_span(f64::NAN), None);
        // JSON has no computable per-sample size, so no rung can be
        // shown to fit.
        assert_eq!(
            max_sustainable_span_for_format(1e9, StreamFormat::Json),
            None
        );
    }

    /// The device-anchored remedy: rungs are reached by halving the rate
    /// the device itself reported, so they exist on its ladder whatever
    /// the receiver clock — including full-V6 rungs the default-clock
    /// ladder does not have.
    #[test]
    fn the_device_anchored_remedy_stays_on_the_device_ladder() {
        // A full V6 at 163.84 MS/s (655 MB/s int16) over a 500 MB/s
        // path: the default-clock ladder tops out at 61.44 MS/s and
        // would give up a third of the sustainable span. Halving the
        // device's own rate lands on its real 81.92 MS/s rung
        // (327.7 MB/s), which fits.
        assert_eq!(
            max_sustainable_sample_rate_below(163.84e6, 500e6, StreamFormat::Int16),
            Some(81.92e6)
        );

        // The measured ECO case: 15.36 MS/s needs 61.44 MB/s; a 55 MB/s
        // path fits the next rung down.
        assert_eq!(
            max_sustainable_sample_rate_below(15_360_000.0, 55e6, StreamFormat::Int16),
            Some(7_680_000.0)
        );

        // A link too narrow for even 1/512 of the device rate has no
        // remedy on the ladder — and JSON has no computable budget.
        assert_eq!(
            max_sustainable_sample_rate_below(1e6, 100.0, StreamFormat::Int16),
            None
        );
        assert_eq!(
            max_sustainable_sample_rate_below(15.36e6, 55e6, StreamFormat::Json),
            None
        );
        assert_eq!(
            max_sustainable_sample_rate_below(0.0, 55e6, StreamFormat::Int16),
            None
        );
    }

    /// The synthetic connect trace both settle tests replay: a 60 MB
    /// backlog burst inside the first 300 ms (~200 MB/s), then the real
    /// stream at 100 kB every 10 ms — 10 MB/s.
    ///
    /// One definition, because the second test's whole premise is that
    /// it replays *the same trace* with the settle window removed: two
    /// hand-maintained copies could drift and the pair would silently
    /// stop being a controlled experiment.
    fn burst_then_steady_trace() -> impl Iterator<Item = (u64, usize)> {
        let burst = [
            (100u64, 20_000_000usize),
            (200, 20_000_000),
            (300, 20_000_000),
        ];
        let steady = (60..=200).map(|tick| (tick * 10, 100_000usize));
        burst.into_iter().chain(steady)
    }

    /// Drive `meter` over a synthetic trace of `(ms, bytes)`
    /// observations until the window closes.
    fn drive(meter: &mut ThroughputMeter, t0: Instant, trace: impl Iterator<Item = (u64, usize)>) {
        for (ms, bytes) in trace {
            if meter.observe(t0 + Duration::from_millis(ms), bytes) {
                break;
            }
        }
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

        let mut meter = ThroughputMeter::starting_at(t0, settle, window);
        drive(&mut meter, t0, burst_then_steady_trace());

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
        // The reported settle is the interval actually discarded — up to
        // the mark at t=600 ms, not the configured 500 ms.
        assert_eq!(m.settle, Duration::from_millis(600));
        assert_eq!(m.stream_sample_rate, None, "no packet header was parsed");
    }

    /// The same trace with no settle window, to show the defect is real
    /// and that the constant is what prevents it. A probe like this
    /// would report a path several times faster than the link and wave
    /// through a span the link cannot carry.
    #[test]
    fn without_a_settle_window_the_same_trace_measures_several_times_too_fast() {
        let t0 = Instant::now();
        let mut meter = ThroughputMeter::starting_at(t0, Duration::ZERO, Duration::from_secs(1));
        drive(&mut meter, t0, burst_then_steady_trace());
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

    /// `finish()` on a window that has not closed is `None`, however
    /// much was counted: a partial count presented as a sustained rate
    /// is exactly the "failed measurement becomes a verdict" mistake the
    /// module exists to prevent — and the contract lives in `finish()`
    /// itself, not in a convention each caller must remember.
    #[test]
    fn an_unfinished_window_is_not_a_measurement() {
        let t0 = Instant::now();
        let mut meter =
            ThroughputMeter::starting_at(t0, Duration::from_millis(500), Duration::from_secs(2));
        // Plenty of counted observations, but the window never closes.
        for tick in 60..=100u64 {
            meter.observe(t0 + Duration::from_millis(tick * 10), 100_000);
        }
        assert!(!meter.is_complete());
        assert!(meter.finish().is_none());
    }

    /// The observation that closes the window is excluded from the
    /// count in both directions — bytes and time — so the rate is the
    /// average over exactly the counted interval.
    #[test]
    fn the_observation_that_closes_the_window_is_not_counted() {
        let t0 = Instant::now();
        let mut meter = ThroughputMeter::starting_at(t0, Duration::ZERO, Duration::from_secs(1));
        meter.observe(t0, 0); // mark
        meter.observe(t0 + Duration::from_millis(500), 5_000_000);
        // A huge batch right on the boundary: counting it at the closing
        // instant would double the answer.
        assert!(meter.observe(t0 + Duration::from_secs(1), 100_000_000));
        let m = meter.finish().expect("complete");
        assert_eq!(m.bytes, 5_000_000);
        // Exactly half the configured window: the floor `finish()` admits
        // (it refuses strictly less), pinned here on purpose.
        assert_eq!(m.window, Duration::from_millis(500));
        assert_eq!(m.byte_rate, 10e6);
    }

    /// A consumer that stalls between observations must not dilute the
    /// rate: the counting used to run to whenever the next observation
    /// happened to arrive, so an 8 s scheduler stall turned a healthy
    /// 60 MB/s stream into a reported ~1.5 MB/s and a spurious "path
    /// cannot carry this span" warning.
    #[test]
    fn a_stalled_consumer_does_not_dilute_the_rate() {
        let t0 = Instant::now();
        let mut meter =
            ThroughputMeter::starting_at(t0, Duration::from_millis(500), Duration::from_secs(2));
        meter.observe(t0 + Duration::from_millis(600), 600_000); // mark
        // A healthy second of stream: 6 MB every 100 ms — 60 MB/s.
        for tick in 7..=16u64 {
            meter.observe(t0 + Duration::from_millis(tick * 100), 6_000_000);
        }
        // The consumer goes away for 8 seconds, then observes again.
        assert!(meter.observe(t0 + Duration::from_millis(9_600), 6_000_000));
        let m = meter.finish().expect("complete");
        // 60 MB over the 1.0 s actually observed — 60 MB/s, not
        // 66 MB / 9.0 s ≈ 7 MB/s.
        assert!((m.byte_rate - 60e6).abs() < 1.0, "got {} B/s", m.byte_rate);
        // Exactly half the configured window — the floor `finish()`
        // admits, pinned deliberately; the test below covers less.
        assert_eq!(m.window, Duration::from_millis(1_000));
    }

    /// A window that closed after observing less than half its
    /// configured span is a failed measurement, not a rate: a couple of
    /// packets and then silence until a stray late observation slams the
    /// window shut says nothing about what the path sustains, and the
    /// stall-tolerant counting above would otherwise happily rate-ify
    /// those 200 ms.
    #[test]
    fn a_window_observed_too_thinly_is_not_a_measurement() {
        let t0 = Instant::now();
        let mut meter =
            ThroughputMeter::starting_at(t0, Duration::from_millis(500), Duration::from_secs(2));
        meter.observe(t0 + Duration::from_millis(600), 600_000); // mark
        meter.observe(t0 + Duration::from_millis(700), 6_000_000);
        meter.observe(t0 + Duration::from_millis(800), 6_000_000);
        // Nothing more until an observation far past the window's end
        // closes it: only 200 ms of a 2 s window was actually observed.
        assert!(meter.observe(t0 + Duration::from_millis(9_000), 6_000_000));
        assert!(meter.is_complete());
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
        meter.observe(t0 + Duration::from_millis(50), 1_000_000);
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
        assert_eq!(
            m.max_sustainable_span_hz(DEFAULT_LINK_FORMAT),
            Some(6_144_000.0)
        );
    }

    /// A serialized RTSA packet header, as the wire carries it, followed
    /// by the LF+RS separator the live server sends.
    fn wire_header(
        payload: PayloadType,
        sample_frequency: Option<f64>,
        start_time: f64,
        end_time: f64,
    ) -> Vec<u8> {
        let metadata = crate::http_streaming::PacketMetadata {
            start_time,
            end_time,
            start_time_day: None,
            end_time_day: None,
            start_frequency: 95e6,
            end_frequency: 105e6,
            sample_frequency,
            samples: 4096,
            unit: "volt".to_string(),
            payload,
            min_power: -120,
            max_power: 0,
            sample_depth: None,
            sample_size: 2,
            scale: None,
            antenna: None,
            categories: None,
            compression: None,
        };
        let mut bytes = serde_json::to_vec(&metadata).expect("serializable");
        bytes.push(ASCII_LINE_FEED);
        bytes.push(ASCII_RECORD_SEPARATOR);
        bytes
    }

    /// The sniff reads the rate out of a header without any payload —
    /// even with binary garbage (containing stray `{` bytes) ahead of
    /// it, even when a degenerate zero-rate status header comes first,
    /// and even when a spectra header with a perfectly positive *frame*
    /// rate comes before the first IQ header.
    #[test]
    fn the_header_sniff_finds_the_rate_and_skips_degenerate_headers() {
        let mut stream = vec![0x00, b'{', 0x7F, ASCII_RECORD_SEPARATOR, 0x42];
        // A status-ish header: no sampleFrequency and zero duration, so
        // its derived rate is 0.0 — it must not end the search.
        stream.extend_from_slice(&wire_header(PayloadType::Iq, None, 100.0, 100.0));
        stream.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]); // its "payload"
        // A spectra header on a mixed mission: no sampleFrequency, so
        // the derived rate is 4096 frames / 50 ms = 81.92 kS/s — positive,
        // and four orders of magnitude off the IQ rate. Not a yardstick.
        stream.extend_from_slice(&wire_header(PayloadType::Spectra, None, 100.0, 100.05));
        stream.extend_from_slice(&[0x01, 0x02, 0x03, 0x04]); // its "payload"
        stream.extend_from_slice(&wire_header(
            PayloadType::Iq,
            Some(15_360_000.0),
            100.0,
            100.001,
        ));

        assert_eq!(RateSniffer::new().feed(&stream), Some(15_360_000.0));

        // A spectra header carrying an explicit sampleFrequency is still
        // not the IQ stream's header: the rule is the payload type, not
        // the field's presence.
        let spectra_only = wire_header(PayloadType::Spectra, Some(15_360_000.0), 100.0, 100.05);
        assert_eq!(RateSniffer::new().feed(&spectra_only), None);

        // No header at all: nothing to report, not a wrong answer.
        assert_eq!(RateSniffer::new().feed(b"not this protocol"), None);

        // A header whose terminator has not arrived yet reports nothing —
        // and the sniffer is incremental, so the rest of the header
        // landing in a later chunk completes the find.
        let complete = wire_header(PayloadType::Iq, Some(1e6), 0.0, 1.0);
        let (first, rest) = complete.split_at(complete.len() - 8);
        let mut sniffer = RateSniffer::new();
        assert_eq!(sniffer.feed(first), None);
        assert_eq!(sniffer.feed(rest), Some(1e6));
    }

    /// Stream from a socket the way the RTSA server does: a backlog
    /// dumped at connect as fast as the socket takes it, an optional
    /// pause, then a steady rate.
    ///
    /// Returns the base URL. The listener serves exactly one connection
    /// and stops writing after `total`, so a probe that never finishes
    /// hits its stall timeout rather than hanging the suite.
    async fn spawn_bursty_stream_server(
        backlog_bytes: usize,
        pause_after_backlog: Duration,
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
            tokio::time::sleep(pause_after_backlog).await;
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
    /// settle window is anchored at the response and that what is
    /// counted is the steady stream. The server dumps 4 MB at connect
    /// (which on loopback lands in milliseconds, so counting it would
    /// read hundreds of MB/s), goes quiet, and then holds 20 MB/s.
    ///
    /// The quiet gap plus a stretched settle window (passed explicitly —
    /// the parameter exists for servers whose backlog outlasts the
    /// default) keeps this deterministic on a loaded runner: the whole
    /// backlog must transit before the settle window ends, however
    /// slowly the client is scheduled or the bytes are processed. The
    /// margins are deliberately fat — a coverage-instrumented CI run was
    /// measured pushing a 40 MB backlog past a 1 s settle, which put the
    /// mark on the backlog's tail and the quiet gap *inside* the window,
    /// reading ~0.5 MB/s off a 20 MB/s stream; 4 MB against 2 s means
    /// even a client processing at a leisurely 2 MB/s clears it.
    #[tokio::test]
    async fn the_probe_measures_the_steady_rate_not_the_connect_backlog() {
        let settle = Duration::from_secs(2);
        let url = spawn_bursty_stream_server(
            4_000_000,
            Duration::from_secs(3),
            200_000,
            Duration::from_millis(10),
            Duration::from_secs(2),
        )
        .await;

        let params = StreamParamsBuilder::new()
            .format(DEFAULT_LINK_FORMAT)
            .build();
        let m = measure_link_throughput_with(
            &url,
            Duration::from_millis(300),
            AuthMethod::None,
            &params,
            settle,
        )
        .await
        .expect("the probe should complete against a stream that keeps sending");

        assert!(
            m.byte_rate < 40e6,
            "measured {} B/s — the 4 MB connect backlog is in the answer",
            m.byte_rate
        );
        assert!(
            m.byte_rate > 5e6,
            "measured {} B/s — the steady ~20 MB/s stream is missing from the answer",
            m.byte_rate
        );
        assert!(
            m.settle_bytes >= 4_000_000,
            "the backlog should be reported as discarded, got {}",
            m.settle_bytes
        );
        // The reported settle is the interval actually discarded, which
        // runs to the first observation after the server's quiet gap —
        // at least the configured window, likely longer.
        assert!(m.settle >= settle, "settle was {:?}", m.settle);
        assert!(m.window >= Duration::from_millis(200), "{:?}", m.window);
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
            Duration::ZERO,
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
        let err = measure_link_throughput("file:///etc/passwd", Duration::from_millis(200))
            .await
            .expect_err("non-HTTP schemes must be refused");
        assert!(err.to_string().contains("HTTP/HTTPS"), "got: {err}");

        let err = measure_link_throughput("not a url", Duration::from_millis(200))
            .await
            .expect_err("an unparseable URL must be refused");
        assert!(err.to_string().contains("Invalid base URL"), "got: {err}");
    }

    /// A window too short to out-run the kernel's socket buffers is
    /// refused before anything is fetched: two adjacent buffered reads
    /// microseconds apart would otherwise "measure" gigabytes a second
    /// over a link that cannot sustain one.
    #[tokio::test]
    async fn a_probe_window_below_the_minimum_is_refused() {
        // The URL is never contacted — validation comes first — so a
        // reserved port is fine here.
        let err = measure_link_throughput("http://127.0.0.1:1", Duration::ZERO)
            .await
            .expect_err("a zero-length window must be refused");
        assert!(err.to_string().contains("minimum"), "got: {err}");
    }

    /// `rate_reduction(0)` is refused before the probe opens a socket:
    /// it is not "no reduction", and the same check guards every other
    /// `/stream` opener in the crate.
    #[tokio::test]
    async fn a_probe_with_rate_reduction_zero_is_refused() {
        let params = StreamParamsBuilder::new().rate_reduction(0).build();
        let err = measure_link_throughput_with(
            "http://127.0.0.1:1",
            Duration::from_secs(1),
            AuthMethod::None,
            &params,
            LINK_PROBE_SETTLE,
        )
        .await
        .expect_err("rate_reduction(0) must be refused");
        assert!(matches!(err, Error::Config(_)), "got: {err}");
    }
}
