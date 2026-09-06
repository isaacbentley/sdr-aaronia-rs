//! Advanced HTTP Streaming Formats
//!
//! This module implements the complete RTSA HTTP streaming specification including
//! multi-format data support, persistent connections, and proper protocol parsing.

use crate::{Error, Result};
use bytes::Bytes;
use half::f16;
use num_complex::Complex32;
use serde::{Deserialize, Serialize};
use std::time::Instant;

/// The two packet-header terminators the wire uses — see
/// [`scan_packet_header`] for the framing they define. Crate-visible so
/// tests that hand-build wire packets spell the framing in its own terms.
pub(crate) const ASCII_RECORD_SEPARATOR: u8 = 30;
pub(crate) const ASCII_LINE_FEED: u8 = 10;
/// Full-scale int16 encode multiplier assumed when neither the packet
/// metadata nor the `?scale=N` query supplied one: `int16 = value * 32768`,
/// decoded as `value = raw / 32768`.
const DEFAULT_INT16_ENCODE_SCALE: f64 = 32768.0;

#[derive(Debug, Default, Clone)]
struct StatsCounters {
    packets_parsed: u64,
    bytes_processed: u64,
    samples_decoded: u64,
    parse_errors: u64,
}

/// Performance statistics for HTTP streaming.
#[derive(Debug, Clone, Default)]
pub struct StreamingPerformanceStats {
    pub packets_parsed: u64,
    pub bytes_processed: u64,
    pub samples_decoded: u64,
    pub parse_errors: u64,
    pub samples_per_second: f64,
    pub bytes_per_second: f64,
    pub packet_rate: f64,
}

/// Supported streaming data formats from RTSA specification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StreamFormat {
    /// Pure JSON format (human readable, slower)
    Json,
    /// 16-bit signed integers with scale factor
    Int16,
    /// IEEE 754-2008 half precision (5 exp + 10 mantissa bits)
    Float16,
    /// Full 32-bit floating point
    Float32,
}

impl StreamFormat {
    /// The capture default: what [`crate::http_source::HttpSourceBuilder`]
    /// streams when no format is chosen, what `StreamStats::default`
    /// reports, and what the link-budget convenience helpers assume
    /// ([`crate::link_budget::DEFAULT_LINK_FORMAT`]). One definition, so
    /// those three cannot drift apart — if this ever changes, every
    /// derived byte-rate figure moves with it.
    ///
    /// Distinct from [`crate::http_endpoints::StreamParamsBuilder`]'s
    /// Float32 default, which serves the lossless direct-streaming path.
    pub const CAPTURE_DEFAULT: StreamFormat = StreamFormat::Int16;

    /// Bytes one IQ sample occupies on the wire in this format, or
    /// `None` when the format has no fixed size per sample.
    ///
    /// This is the constant the whole link budget turns on: at 4 bytes a
    /// sample a 30.72 MS/s stream is 123 MB/s, which is past what a
    /// gigabit path delivers, and the server drops what it cannot send.
    /// [`crate::link_budget`] multiplies by it in one direction and
    /// divides by it in the other; `calculate_binary_size` uses it to
    /// find where a packet's payload ends. Those must agree, so there is
    /// one definition.
    ///
    /// A *scalar* payload (spectra, histogram, categories) carries one
    /// value where IQ carries a pair, so it uses half this figure.
    ///
    /// [`Self::Json`] is `None`: its samples are ASCII decimal and have
    /// no fixed width, so "how many bytes is a sample" has no answer —
    /// and the type makes every caller handle that, instead of a `0`
    /// sentinel that a multiplication would silently read as "free".
    pub fn iq_bytes_per_sample(&self) -> Option<usize> {
        match self {
            Self::Int16 => Some(4),   // 2 bytes I + 2 bytes Q
            Self::Float16 => Some(4), // 2 bytes I + 2 bytes Q
            Self::Float32 => Some(8), // 4 bytes I + 4 bytes Q
            Self::Json => None,       // No binary data
        }
    }

    /// Wire-format name used in the `?format=` query parameter.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Int16 => "int16",
            Self::Float16 => "float16",
            Self::Float32 => "float32",
        }
    }

    /// Backwards-compatible alias for `<Self as FromStr>::from_str`.
    ///
    /// A `pub fn from_str(&str) -> Result<Self>` inherent method
    /// existed here long before the canonical `FromStr` impl below
    /// did; this delegate keeps every existing call site
    /// (`StreamFormat::from_str("json")`, both in this crate's tests
    /// and downstream) compiling unchanged. Callers that prefer the
    /// idiomatic `"json".parse::<StreamFormat>()` route through the
    /// trait.
    ///
    /// `clippy::should_implement_trait` fires here because the
    /// signature collides with `FromStr::from_str` — the underlying
    /// concern (no trait, only an inherent method with this name) was
    /// already addressed by the `impl FromStr` below in v0.4.9 / A29.
    /// The allow is preserved to keep this no-op alias intentional
    /// rather than accidental.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self> {
        <Self as std::str::FromStr>::from_str(s)
    }
}

impl std::str::FromStr for StreamFormat {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "int16" => Ok(Self::Int16),
            "float16" => Ok(Self::Float16),
            "float32" => Ok(Self::Float32),
            _ => Err(Error::Protocol(format!("Unsupported stream format: {}", s))),
        }
    }
}

/// Data payload types supported by RTSA streaming
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum PayloadType {
    /// IQ sample data
    Iq,
    /// Spectrum analysis data
    Spectra,
    /// Histogram data
    Histogram,
    /// Channel power/category data
    Categories,
}

/// Antenna specification for directional measurements.
///
/// Real RTSA HTTP servers omit fields they have no data for — a
/// SpectranV6 HTTP-server mission emits `"antenna":{"name":""}` with no
/// position at all. Every field therefore defaults; requiring them made
/// packet-metadata deserialization fail on every packet from real
/// hardware, which stalled the stream parser in an endless resync.
///
/// Position/orientation fields are `Option` so "not reported" is
/// distinguishable from a genuine 0.0 (which for lat/lon is a real
/// place).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AntennaSpec {
    pub name: String,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub azimuth: Option<f64>,
    pub declination: Option<f64>,
}

/// Category specification for channel power measurements
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorySpec {
    pub name: String,
    pub start_frequency: f64,
    pub end_frequency: f64,
}

/// Deserializer for the `samples` field of [`PacketMetadata`].
///
/// The HTTP stream wire format uses the same `samples` JSON key with
/// two different meanings depending on the stream format:
///
/// * Binary streams (Int16 / Float16 / Float32) put the IQ-pair count
///   here as a JSON number.
/// * JSON streams put the array of sample values here instead, with
///   the pair count implicit in `array.len() / 2` for IQ payloads.
///
/// This helper accepts either form and stores a `u64` count in
/// `PacketMetadata.samples`. For the array form, the stored value is
/// the raw element count — `parse_json_chunk` later overwrites it with
/// the decoded `Complex32` count so downstream stats see the
/// IQ-pair-aware number.
fn deserialize_samples_count<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{Error, SeqAccess, Visitor};
    use std::fmt;

    struct SamplesVisitor;

    impl<'de> Visitor<'de> for SamplesVisitor {
        type Value = u64;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(
                f,
                "either a non-negative integer (count form) or an array (JSON-stream value form)"
            )
        }

        fn visit_u64<E>(self, v: u64) -> Result<u64, E> {
            Ok(v)
        }

        fn visit_i64<E: Error>(self, v: i64) -> Result<u64, E> {
            u64::try_from(v).map_err(|_| E::custom(format!("negative samples count: {v}")))
        }

        fn visit_f64<E: Error>(self, v: f64) -> Result<u64, E> {
            if v.is_finite() && v >= 0.0 && v <= u64::MAX as f64 {
                Ok(v as u64)
            } else {
                Err(E::custom(format!(
                    "samples count is not a non-negative finite number: {v}"
                )))
            }
        }

        fn visit_seq<S>(self, mut seq: S) -> Result<u64, S::Error>
        where
            S: SeqAccess<'de>,
        {
            let mut n: u64 = 0;
            while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                n = n.saturating_add(1);
            }
            Ok(n)
        }
    }

    deserializer.deserialize_any(SamplesVisitor)
}

/// Complete packet metadata from RTSA HTTP stream
/// Enhanced to capture all real-time streaming parameters
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PacketMetadata {
    /// Start time in seconds since Unix epoch
    pub start_time: f64,
    /// End time in seconds since Unix epoch
    pub end_time: f64,
    /// Start time as day offset (seconds from day start)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time_day: Option<f64>,
    /// End time as day offset (seconds from day start)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_time_day: Option<f64>,
    /// Start of frequency range in Hz
    pub start_frequency: f64,
    /// End of frequency range in Hz
    pub end_frequency: f64,
    /// Sample frequency/rate in Hz
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_frequency: Option<f64>,
    /// Number of IQ sample pairs in this packet.
    ///
    /// On the wire this field can arrive as either a JSON number (binary
    /// streams) or a JSON array of values (JSON streams). The custom
    /// deserializer collapses both forms to a `u64` count; for JSON
    /// streams, `parse_json_chunk` overwrites the value post-parse with
    /// the actual `Complex32` count after decoding.
    #[serde(deserialize_with = "deserialize_samples_count")]
    pub samples: u64,
    /// Unit of sample values (volt, dbm, generic, percentage)
    pub unit: String,
    /// Type of data payload
    pub payload: PayloadType,
    /// Minimum power level
    pub min_power: i32,
    /// Maximum power level
    pub max_power: i32,
    /// Number of sample sets per sample (e.g. histogram bins)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_depth: Option<u32>,
    /// Number of components per sample (2 for IQ)
    pub sample_size: u32,
    /// Scale factor for data conversion
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    /// Antenna specification
    #[serde(skip_serializing_if = "Option::is_none")]
    pub antenna: Option<AntennaSpec>,
    /// Category specifications for channel data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<CategorySpec>>,
    /// Compression factor for spectrum data (0 = uncompressed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compression: Option<u32>,
}

/// Real-time SDR configuration derived from streaming metadata
#[derive(Debug, Clone, PartialEq)]
pub struct StreamingSdrConfig {
    /// Current center frequency in Hz
    pub center_frequency: f64,
    /// Total bandwidth being captured in Hz
    pub bandwidth: f64,
    /// Sample rate in Hz
    pub sample_rate: f64,
    /// Current reference level range
    pub power_range: (i32, i32), // (min, max)
    /// Physical unit of measurements
    pub unit: String,
    /// Data format being streamed
    pub data_format: String,
    /// Antenna configuration
    pub antenna_name: String,
    /// Packet timing statistics
    pub packet_duration_ms: f64,
    /// Samples per packet
    pub samples_per_packet: u64,
}

impl PacketMetadata {
    /// The rate this packet reports, in Hz: `sampleFrequency` when the
    /// header carries it, else `samples / duration`, else `0.0`.
    ///
    /// The fallback is only a *sample* rate for an IQ packet. A spectra
    /// or histogram header counts frames in `samples`, so there it is a
    /// frame rate — orders of magnitude below the IQ rate — and a
    /// consumer wanting the device's IQ rate must check `payload` first.
    /// The zero-duration guard keeps a degenerate status header (equal
    /// start and end times) from producing Inf/NaN.
    pub fn sample_rate(&self) -> f64 {
        let duration = self.end_time - self.start_time;
        self.sample_frequency.unwrap_or(if duration > 0.0 {
            self.samples as f64 / duration
        } else {
            0.0
        })
    }
}

impl StreamingSdrConfig {
    /// Create SDR config from packet metadata
    pub fn from_metadata(metadata: &PacketMetadata) -> Self {
        let center_freq = (metadata.start_frequency + metadata.end_frequency) / 2.0;
        let bandwidth = metadata.end_frequency - metadata.start_frequency;
        let duration = metadata.end_time - metadata.start_time;
        let duration_ms = duration * 1000.0;
        let sample_rate = metadata.sample_rate();
        let antenna_name = metadata
            .antenna
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "Unknown".to_string());

        Self {
            center_frequency: center_freq,
            bandwidth,
            sample_rate,
            power_range: (metadata.min_power, metadata.max_power),
            unit: metadata.unit.clone(),
            data_format: format!("{:?}", metadata.payload),
            antenna_name,
            packet_duration_ms: duration_ms,
            samples_per_packet: metadata.samples,
        }
    }

    /// Get human-readable frequency range
    pub fn frequency_range_mhz(&self) -> (f64, f64) {
        let start_mhz = (self.center_frequency - self.bandwidth / 2.0) / 1e6;
        let end_mhz = (self.center_frequency + self.bandwidth / 2.0) / 1e6;
        (start_mhz, end_mhz)
    }

    /// Get sample rate in MHz
    pub fn sample_rate_mhz(&self) -> f64 {
        self.sample_rate / 1e6
    }

    /// Get bandwidth in MHz
    pub fn bandwidth_mhz(&self) -> f64 {
        self.bandwidth / 1e6
    }
}

/// Outcome of [`scan_packet_header`]: a complete header, or how much of
/// the buffer is spent while waiting for more data.
pub(crate) enum HeaderScan {
    /// A complete header. Boxed because the metadata is ~an order of
    /// magnitude wider than the other variant, which is the common case
    /// while a header arrives.
    Header {
        metadata: Box<PacketMetadata>,
        /// The header's JSON text, as a byte range of the scanned buffer.
        json: std::ops::Range<usize>,
        /// Where the payload begins: past the terminator, and past the
        /// record separator that follows a line feed on the real wire.
        /// Everything before it is spent.
        payload_start: usize,
        /// `{`-candidates that failed to parse as a header on the way.
        rejected: usize,
    },
    /// No complete header in the buffer. The first `skippable` bytes can
    /// never begin one — every candidate there has already failed — and
    /// may be discarded; anything after may be a header still arriving.
    Incomplete { skippable: usize, rejected: usize },
}

/// Scan `buf` for the first complete packet JSON header, decoding no
/// payload.
///
/// **The one framing implementation**: `StreamParser` frames every
/// packet through here, and the link-budget probe's header sniff uses it
/// to learn the device's rate without buffering or decoding payloads.
///
/// Wire format: `{JSON}<sep>[PAYLOAD]`, where `<sep>` is a record
/// separator (`0x1E`) or a line feed (`0x0A`). Verified against live
/// SpectranV6 hardware (float32/int16, iq and spectra payloads): the
/// real wire format is `{json}\n\x1e<binary>` — the JSON line ends with
/// LF and the payload is prefixed with RS, i.e. **two** separator bytes.
/// Treating the LF as the sole separator shifted every binary payload by
/// one byte and decoded pure garbage. A lone LF or lone RS is still
/// accepted for spec-conservative peers; and *exactly* one of each — an
/// earlier revision skipped a *run* of separator bytes, which swallowed
/// payloads whose first byte happened to be `0x1E`/`0x0A`.
///
/// **Resync**: a candidate is a `{` followed by a terminator; valid JSON
/// cannot contain a raw `0x1E`/`0x0A` (control characters must be
/// escaped), so a terminated candidate either parses now or never will.
/// One that fails was not a header (mid-stream corruption, or a brace
/// inside a lost packet's binary data), and the scan resyncs **one byte
/// past its `{`** — binary payloads contain separator bytes too, so a
/// real header can share a terminator with a stray `{` ahead of it, and
/// a coarser skip would jump straight over it. Every candidate between
/// one terminator and the next shares that terminator, so it is found
/// once per region rather than once per candidate: the scan is linear
/// in the buffer, whatever the brace density of the garbage.
pub(crate) fn scan_packet_header(buf: &[u8]) -> HeaderScan {
    let mut rejected = 0;
    let mut pos = 0;
    loop {
        let Some(start) = buf[pos..].iter().position(|&b| b == b'{') else {
            // No candidate start remains; a header must begin at a `{`,
            // so the whole buffer is spent.
            return HeaderScan::Incomplete {
                skippable: buf.len(),
                rejected,
            };
        };
        let start = pos + start;
        let Some(end) = buf[start..]
            .iter()
            .position(|&b| b == ASCII_RECORD_SEPARATOR || b == ASCII_LINE_FEED)
        else {
            // A candidate whose terminator has not arrived yet; keep it,
            // spend everything before it.
            return HeaderScan::Incomplete {
                skippable: start,
                rejected,
            };
        };
        let end = start + end;

        // Every `{` in `start..end` is a candidate ending at `end`.
        let mut candidate = start;
        loop {
            if let Ok(metadata) = serde_json::from_slice::<PacketMetadata>(&buf[candidate..end]) {
                let mut payload_start = end + 1;
                if buf[end] == ASCII_LINE_FEED
                    && buf.get(payload_start) == Some(&ASCII_RECORD_SEPARATOR)
                {
                    payload_start += 1;
                }
                return HeaderScan::Header {
                    metadata: Box::new(metadata),
                    json: candidate..end,
                    payload_start,
                    rejected,
                };
            }
            rejected += 1;
            match buf[candidate + 1..end].iter().position(|&b| b == b'{') {
                Some(next) => candidate += 1 + next,
                None => break,
            }
        }
        // Nothing in this region was a header; the terminator is spent
        // with it.
        pos = end + 1;
    }
}

/// Parsed streaming packet with metadata and sample data
#[derive(Debug, Clone)]
pub struct StreamPacket {
    pub metadata: PacketMetadata,
    pub samples: Vec<Complex32>,
    /// Derived SDR configuration from this packet
    pub sdr_config: StreamingSdrConfig,
}

impl StreamPacket {
    /// Create a new stream packet with derived SDR config
    pub fn new(metadata: PacketMetadata, samples: Vec<Complex32>) -> Self {
        let sdr_config = StreamingSdrConfig::from_metadata(&metadata);
        Self {
            metadata,
            samples,
            sdr_config,
        }
    }

    /// Get timing information for this packet
    pub fn get_timing_info(&self) -> (f64, f64, f64) {
        (
            self.metadata.start_time,
            self.metadata.end_time,
            self.sdr_config.packet_duration_ms,
        )
    }

    /// Check if this packet contains a specific frequency
    pub fn contains_frequency(&self, freq_hz: f64) -> bool {
        freq_hz >= self.metadata.start_frequency && freq_hz <= self.metadata.end_frequency
    }
}

/// Timestamp-gap drop detector.
///
/// The v9 RTSA HTTP Stream Server Endpoints document warns that "the RTSA
/// HTTP server block will start dropping data when the outbound TCP buffer
/// exceeds 8 Mbytes" and notes that "a loss of data can be determined by
/// comparing the timestamps of two adjacent data packets". `DropDetector`
/// implements that check: feed it consecutive packets and it reports the
/// gap between this packet's `start_time` and the previous packet's
/// `end_time`. Gaps longer than `tolerance` are returned as a drop event.
#[derive(Debug, Clone)]
pub struct DropDetector {
    /// Maximum tolerated `start_time - prev.end_time` before reporting a drop.
    pub tolerance: f64,
    /// Last seen `end_time`, in seconds since the Unix epoch.
    last_end_time: Option<f64>,
    /// Cumulative number of drops detected.
    drops: u64,
    /// Cumulative gap (in seconds) attributed to drops.
    cumulative_gap: f64,
}

/// One-shot result from feeding a packet to `DropDetector`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DropResult {
    /// First packet ever seen, or contiguous with the prior packet.
    Continuous,
    /// A gap larger than `tolerance` was detected. `gap_seconds` is the
    /// span between this packet's `start_time` and the previous packet's
    /// `end_time`.
    Drop {
        /// Length of the detected gap, in seconds.
        gap_seconds: f64,
    },
}

impl Default for DropDetector {
    fn default() -> Self {
        // Default tolerance: 1 millisecond. Smaller than every practical
        // packet duration we've seen but large enough to absorb f64 round-off.
        Self::new(1e-3)
    }
}

impl DropDetector {
    /// Create a detector with the given gap tolerance, in seconds.
    pub fn new(tolerance: f64) -> Self {
        Self {
            tolerance,
            last_end_time: None,
            drops: 0,
            cumulative_gap: 0.0,
        }
    }

    /// Inspect a packet's timing info and return whether the stream is
    /// still continuous.
    pub fn observe(&mut self, packet: &StreamPacket) -> DropResult {
        let start = packet.metadata.start_time;
        let end = packet.metadata.end_time;
        let result = match self.last_end_time {
            Some(prev_end) => {
                let gap = start - prev_end;
                if gap > self.tolerance {
                    self.drops += 1;
                    self.cumulative_gap += gap;
                    DropResult::Drop { gap_seconds: gap }
                } else {
                    DropResult::Continuous
                }
            }
            None => DropResult::Continuous,
        };
        self.last_end_time = Some(end);
        result
    }

    /// Cumulative number of drops detected so far.
    pub fn drops(&self) -> u64 {
        self.drops
    }

    /// Cumulative gap time, in seconds, attributed to detected drops.
    pub fn cumulative_gap_seconds(&self) -> f64 {
        self.cumulative_gap
    }

    /// Clear all accumulated drop statistics and forget the last
    /// observed packet.
    pub fn reset(&mut self) {
        self.last_end_time = None;
        self.drops = 0;
        self.cumulative_gap = 0.0;
    }

    /// Forget the last observed packet while **keeping** the accumulated
    /// drop statistics.
    ///
    /// For use across a deliberate discontinuity — a stream reconnect,
    /// say — where the next packet's timestamp bears no relation to the
    /// previous one and would otherwise be reported as one enormous
    /// drop. [`Self::reset`] would zero the counters instead, making the
    /// cumulative total that consumers read as monotonic jump backwards
    /// and corrupting anything computing deltas from it.
    pub fn resync(&mut self) {
        self.last_end_time = None;
    }
}

/// Simple HTTP stream parser for Aaronia RTSA format
pub struct StreamParser {
    format: StreamFormat,
    buffer: Vec<u8>,
    /// Number of already-consumed bytes at the front of `buffer`. Consumed
    /// data is compacted away lazily (see `consume_buffer`) so byte-at-a-
    /// time resync over a corrupt region doesn't trigger an O(n²) memmove
    /// cascade.
    consumed: usize,
    /// The server-side encode scale requested via `?scale=N`, used as the
    /// fallback when a packet's `scale` field is absent. **Semantics
    /// (verified on live hardware)**: `scale` is the multiplier the server
    /// applies when quantising, `int16 = round(value * scale)`, so decoding
    /// *divides*: `value = raw / scale`. (A live dBm spectra packet with
    /// `scale: 100` carries raw values like `-11378` = −113.78 dBm; the
    /// earlier implementation multiplied instead, producing values wrong by
    /// a factor of scale².)
    default_encode_scale: Option<f64>,
    counters: StatsCounters,
    started_at: Instant,
}

impl StreamParser {
    /// `scale` is the server-side `?scale=N` encode multiplier, if one was
    /// requested — see `Self::int16_decode_scale` for the semantics.
    pub fn new(format: StreamFormat, scale: Option<f64>) -> Result<Self> {
        Ok(Self {
            format,
            buffer: Vec::new(),
            consumed: 0,
            default_encode_scale: scale,
            counters: StatsCounters::default(),
            started_at: Instant::now(),
        })
    }

    /// Snapshot the live performance counters.
    pub fn stats(&self) -> StreamingPerformanceStats {
        let elapsed = self.started_at.elapsed().as_secs_f64().max(1e-9);
        StreamingPerformanceStats {
            packets_parsed: self.counters.packets_parsed,
            bytes_processed: self.counters.bytes_processed,
            samples_decoded: self.counters.samples_decoded,
            parse_errors: self.counters.parse_errors,
            samples_per_second: self.counters.samples_decoded as f64 / elapsed,
            bytes_per_second: self.counters.bytes_processed as f64 / elapsed,
            packet_rate: self.counters.packets_parsed as f64 / elapsed,
        }
    }

    /// Reset counters and the elapsed-time origin.
    pub fn reset_stats(&mut self) {
        self.counters = StatsCounters::default();
        self.started_at = Instant::now();
    }

    /// The wire's encode scale for this packet: the per-packet `scale`
    /// field, else the `?scale=N` the stream was opened with, else the
    /// full-scale default (32768).
    fn int16_encode_scale(&self, metadata: &PacketMetadata) -> f64 {
        metadata
            .scale
            .or(self.default_encode_scale)
            .filter(|s| s.is_finite() && *s > 0.0)
            .unwrap_or(DEFAULT_INT16_ENCODE_SCALE)
    }

    /// Multiplier that decodes a raw `i16` to its value:
    /// `value = raw * int16_decode_scale(..)` = `raw / encode_scale`.
    fn int16_decode_scale(&self, metadata: &PacketMetadata) -> f32 {
        (1.0 / self.int16_encode_scale(metadata)) as f32
    }

    /// Process incoming stream data and return completed packets.
    ///
    /// This is the only parsing entry point: it buffers across HTTP chunk
    /// boundaries and returns *every* packet completed by `data` (an HTTP
    /// chunk routinely contains zero, one, or several packets). The old
    /// `parse_chunk` API — which returned at most one packet per chunk and
    /// erred on partials — was removed because it silently dropped the
    /// remaining packets in multi-packet chunks.
    pub fn process_data(&mut self, data: &Bytes) -> Result<Vec<StreamPacket>> {
        self.counters.bytes_processed += data.len() as u64;
        self.buffer.extend_from_slice(data);

        let mut completed_packets = Vec::new();
        loop {
            match self.try_parse_complete_packet() {
                Ok(Some(packet)) => completed_packets.push(packet),
                Ok(None) => break,
                Err(e) => {
                    self.counters.parse_errors += 1;
                    return Err(e);
                }
            }
        }

        Ok(completed_packets)
    }

    /// Upper bound on how much un-parseable data we buffer while waiting
    /// for a packet's JSON header to complete. Past this, the stream is
    /// declared corrupt rather than growing without bound.
    const MAX_JSON_BUFFER: usize = 10 * 1024 * 1024;

    /// Upper bound on a single packet's binary payload. Metadata is
    /// attacker-/corruption-controlled, so an insane `samples` count must
    /// error instead of making the parser wait forever (and buffer
    /// gigabytes) for a payload that will never arrive.
    const MAX_BINARY_PAYLOAD: usize = 256 * 1024 * 1024;

    /// Compact the buffer once this many consumed bytes have accumulated.
    /// Deferring the memmove keeps single-byte resync steps O(1) amortised
    /// instead of shifting up to `MAX_JSON_BUFFER` bytes per skipped byte.
    const COMPACT_THRESHOLD: usize = 64 * 1024;

    /// Un-consumed bytes: the parser's working view of the buffer.
    fn pending(&self) -> &[u8] {
        &self.buffer[self.consumed..]
    }

    /// Mark `n` pending bytes (a parsed packet, or corrupt data being
    /// skipped) as consumed. Compaction is deferred until the consumed
    /// prefix crosses `COMPACT_THRESHOLD`.
    fn consume_buffer(&mut self, n: usize) {
        self.consumed += n;
        if self.consumed >= self.buffer.len() {
            self.buffer.clear();
            self.consumed = 0;
        } else if self.consumed >= Self::COMPACT_THRESHOLD {
            self.buffer.copy_within(self.consumed.., 0);
            let remaining = self.buffer.len() - self.consumed;
            self.buffer.truncate(remaining);
            self.consumed = 0;
        }
    }

    /// Parse one complete packet from the front of the buffer, if present.
    ///
    /// Framing — the header's extent, the one- or two-byte separator, and
    /// the resync past a `{` that is not a header — is
    /// [`scan_packet_header`]'s; this adds the payload: sized from the
    /// header and awaited for the binary formats, carried inside the
    /// header document for the pure-JSON format. Nothing is consumed
    /// until the whole packet is buffered, so a payload whose second
    /// separator byte has not arrived yet is not mis-framed: the re-scan
    /// after the next chunk sees it.
    fn try_parse_complete_packet(&mut self) -> Result<Option<StreamPacket>> {
        let (metadata, json, payload_start) = match scan_packet_header(self.pending()) {
            HeaderScan::Incomplete {
                skippable,
                rejected,
            } => {
                self.counters.parse_errors += rejected as u64;
                // Spent bytes are a cheap offset bump (no memmove), so
                // walking through a corrupt region stays linear overall.
                self.consume_buffer(skippable);
                // Bound how long an unterminated header is waited for.
                if self.pending().len() > Self::MAX_JSON_BUFFER {
                    return Err(Error::Protocol(format!(
                        "packet JSON header exceeded {} bytes without terminating",
                        Self::MAX_JSON_BUFFER
                    )));
                }
                return Ok(None);
            }
            HeaderScan::Header {
                metadata,
                json,
                payload_start,
                rejected,
            } => {
                self.counters.parse_errors += rejected as u64;
                (*metadata, json, payload_start)
            }
        };

        // Pure-JSON streams carry the sample values inside the header
        // document, so that path (and only that path) pays for a full
        // `serde_json::Value` DOM. Binary streams — the throughput formats
        // — never build one: the header the scan deserialized is the
        // packet's metadata as is.
        if self.format == StreamFormat::Json {
            // The scan just parsed this text as `PacketMetadata`, so it is
            // valid JSON; the error arm is unreachable but not worth a
            // panic path.
            let json_value = serde_json::from_slice::<serde_json::Value>(&self.pending()[json])
                .map_err(|e| {
                    Error::Protocol(format!("packet header is not a JSON document: {e}"))
                })?;
            let samples = self.parse_json_samples(&metadata, &json_value)?;
            let mut metadata = metadata;
            // The wire `samples` field holds the raw array length; for IQ
            // payloads each Complex32 comes from two entries. Reconcile so
            // downstream stats see the pair-aware count.
            metadata.samples = samples.len() as u64;
            self.counters.packets_parsed += 1;
            self.counters.samples_decoded += samples.len() as u64;
            self.consume_buffer(payload_start);
            return Ok(Some(StreamPacket::new(metadata, samples)));
        }

        let expected_bytes = self.calculate_binary_size(&metadata)?;
        if expected_bytes > Self::MAX_BINARY_PAYLOAD {
            return Err(Error::Protocol(format!(
                "packet declares a binary payload of {} bytes (cap: {})",
                expected_bytes,
                Self::MAX_BINARY_PAYLOAD
            )));
        }
        let binary_end = payload_start + expected_bytes;

        if self.pending().len() < binary_end {
            return Ok(None);
        }

        let samples = {
            let binary_data = &self.pending()[payload_start..binary_end];
            self.parse_binary_samples(&metadata, binary_data)?
        };
        self.counters.packets_parsed += 1;
        self.counters.samples_decoded += samples.len() as u64;
        let packet = StreamPacket::new(metadata, samples);
        self.consume_buffer(binary_end);
        Ok(Some(packet))
    }

    fn calculate_binary_size(&self, metadata: &PacketMetadata) -> Result<usize> {
        // Unreachable for `Json` today — `try_parse_complete_packet`
        // branches to the pure-JSON path first — but enforced rather than
        // assumed: a refactor that let JSON through would otherwise frame
        // zero-length payloads and silently desync the stream.
        let bytes_per_sample: usize = self.format.iq_bytes_per_sample().ok_or_else(|| {
            Error::Protocol("JSON streams have no fixed-size binary payload to frame".to_string())
        })?;

        match metadata.payload {
            PayloadType::Iq => {
                // IQ data: samples is the number of IQ pairs. The count is
                // wire-controlled, so refuse values that don't fit.
                usize::try_from(metadata.samples)
                    .ok()
                    .and_then(|n| n.checked_mul(bytes_per_sample))
                    .ok_or_else(|| {
                        Error::Protocol(format!(
                            "IQ sample count {} overflows payload size",
                            metadata.samples
                        ))
                    })
            }
            PayloadType::Spectra | PayloadType::Histogram => {
                // A packet holds `samples` frames of `sample_size` bins ×
                // `sample_depth` planes, one scalar each. Verified on live
                // SpectranV6 hardware: an IQ-Power-Spectrum packet with
                // samples=64, sampleSize=820, depth=1 carries exactly
                // 64·820·4 bytes of float32 — the earlier formula ignored
                // `samples` and under-read every multi-frame packet 64×,
                // desyncing the stream.
                let depth = metadata.sample_depth.unwrap_or(1).max(1) as usize;
                usize::try_from(metadata.samples)
                    .ok()
                    .and_then(|frames| frames.checked_mul(metadata.sample_size as usize))
                    .and_then(|n| n.checked_mul(depth))
                    .and_then(|n| n.checked_mul(bytes_per_sample / 2))
                    .ok_or_else(|| {
                        Error::Protocol(format!(
                            "spectra frame count {} × {} bins overflows payload size",
                            metadata.samples, metadata.sample_size
                        ))
                    })
            }
            PayloadType::Categories => {
                // Category data: one value per category, per frame. The
                // per-frame multiplication mirrors the spectra layout
                // (inferred — no live categories source available to verify).
                let num_categories = metadata
                    .categories
                    .as_ref()
                    .map(|cats| cats.len())
                    .unwrap_or(metadata.sample_size as usize);
                usize::try_from(metadata.samples.max(1))
                    .ok()
                    .and_then(|frames| frames.checked_mul(num_categories))
                    .and_then(|n| n.checked_mul(bytes_per_sample / 2))
                    .ok_or_else(|| Error::Protocol("categories payload size overflows".to_string()))
            }
        }
    }

    fn parse_binary_samples(
        &self,
        metadata: &PacketMetadata,
        data: &[u8],
    ) -> Result<Vec<Complex32>> {
        match metadata.payload {
            PayloadType::Iq => self.parse_iq_samples(metadata, data),
            PayloadType::Spectra => self.parse_spectrum_samples(metadata, data),
            PayloadType::Histogram => self.parse_histogram_samples(metadata, data),
            PayloadType::Categories => self.parse_category_samples(metadata, data),
        }
    }

    /// Parse binary IQ payload bytes. IQ payloads are never compressed on
    /// the HTTP stream (compression applies to spectrum data only), so the
    /// bytes decode directly per the wire format.
    fn parse_iq_samples(&self, metadata: &PacketMetadata, data: &[u8]) -> Result<Vec<Complex32>> {
        match self.format {
            StreamFormat::Int16 => {
                let scale = self.int16_decode_scale(metadata);
                self.parse_iq_int16_optimized(data, scale)
            }
            StreamFormat::Float16 => self.parse_iq_float16_optimized(data),
            StreamFormat::Float32 => self.parse_iq_float32_optimized(data),
            StreamFormat::Json => Err(Error::Protocol(
                "Binary IQ parsing not applicable for JSON format".to_string(),
            )),
        }
    }

    /// Optimized Int16 IQ parsing with bulk operations and pre-allocation.
    /// `scale` is the decode multiplier — `1 / metadata.scale`, see
    /// [`int16_decode_scale`] — so `f32 = scale * raw_i16`.
    fn parse_iq_int16_optimized(&self, data: &[u8], scale: f32) -> Result<Vec<Complex32>> {
        let num_samples = data.len() / 4;
        let mut samples = Vec::with_capacity(num_samples);

        for chunk in data.as_chunks::<4>().0 {
            let i_raw = i16::from_le_bytes([chunk[0], chunk[1]]);
            let q_raw = i16::from_le_bytes([chunk[2], chunk[3]]);

            samples.push(Complex32::new(
                (i_raw as f32) * scale,
                (q_raw as f32) * scale,
            ));
        }

        Ok(samples)
    }

    /// Optimized Float16 IQ parsing with bulk operations
    fn parse_iq_float16_optimized(&self, data: &[u8]) -> Result<Vec<Complex32>> {
        let num_samples = data.len() / 4;
        let mut samples = Vec::with_capacity(num_samples);

        for chunk in data.as_chunks::<4>().0 {
            let i_raw = f16::from_le_bytes([chunk[0], chunk[1]]);
            let q_raw = f16::from_le_bytes([chunk[2], chunk[3]]);

            samples.push(Complex32::new(i_raw.to_f32(), q_raw.to_f32()));
        }

        Ok(samples)
    }

    /// Optimized Float32 IQ parsing.
    ///
    /// On a little-endian host the wire payload is byte-identical to a
    /// `[Complex32]` (`repr(C) { re: f32, im: f32 }`, little-endian), so the
    /// whole payload is copied in one `memcpy` rather than decoding each
    /// `f32` individually. Big-endian hosts fall back to the portable
    /// per-element decode. The input bytes are unaligned (a subslice of the
    /// parser buffer), so we copy *into* an aligned `Vec<Complex32>` rather
    /// than reinterpreting the source in place.
    fn parse_iq_float32_optimized(&self, data: &[u8]) -> Result<Vec<Complex32>> {
        let num_samples = data.len() / 8;

        #[cfg(target_endian = "little")]
        {
            // `Complex32` is `#[repr(C)] { re: f32, im: f32 }`; pin the layout
            // assumption at compile time.
            const _: () =
                assert!(std::mem::size_of::<Complex32>() == 2 * std::mem::size_of::<f32>());
            let mut samples = Vec::<Complex32>::with_capacity(num_samples);
            // SAFETY: `samples` was allocated for `num_samples` Complex32
            // (= num_samples * 8 bytes), and `data` holds at least that many
            // bytes (`num_samples == data.len() / 8`). A raw byte copy has no
            // alignment requirement on either end, and on little-endian the
            // source bytes are exactly the Complex32 representation, so every
            // byte of the first `num_samples` elements is initialised before
            // `set_len`.
            #[allow(clippy::uninit_vec)]
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    samples.as_mut_ptr() as *mut u8,
                    num_samples * 8,
                );
                samples.set_len(num_samples);
            }
            Ok(samples)
        }

        #[cfg(not(target_endian = "little"))]
        {
            let mut samples = Vec::with_capacity(num_samples);
            for chunk in data.as_chunks::<8>().0 {
                let i_val = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let q_val = f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
                samples.push(Complex32::new(i_val, q_val));
            }
            Ok(samples)
        }
    }

    fn parse_spectrum_samples(
        &self,
        metadata: &PacketMetadata,
        data: &[u8],
    ) -> Result<Vec<Complex32>> {
        use tracing::{debug, warn};

        // Handle compression if present
        let data_to_parse = if let Some(compression_factor) = metadata.compression {
            if compression_factor > 0 {
                debug!(
                    "Decompressing spectrum data: {} bytes with compression factor {}",
                    data.len(),
                    compression_factor
                );

                match self.decompress_spectrum_data(data, compression_factor, metadata) {
                    Ok(decompressed_data) => {
                        debug!(
                            "Successfully decompressed spectrum data: {} -> {} bytes",
                            data.len(),
                            decompressed_data.len()
                        );
                        decompressed_data
                    }
                    Err(e) => {
                        warn!(
                            "Failed to decompress spectrum data (compression factor {}): {:?}.",
                            compression_factor, e
                        );
                        return Err(e);
                    }
                }
            } else {
                debug!("Spectrum data is uncompressed (compression factor 0)");
                data.to_vec()
            }
        } else {
            // No compression metadata - assume uncompressed
            debug!("No compression metadata found - treating as uncompressed spectrum data");
            data.to_vec()
        };

        let data_to_parse = &data_to_parse;

        // For spectrum data, convert to complex with real values and zero imaginary
        let mut samples = Vec::new();

        match self.format {
            StreamFormat::Int16 => {
                let scale = self.int16_decode_scale(metadata);
                for chunk in data_to_parse.as_chunks::<2>().0 {
                    let val_raw = i16::from_le_bytes([chunk[0], chunk[1]]);
                    let val = val_raw as f32 * scale;
                    samples.push(Complex32::new(val, 0.0));
                }
            }
            StreamFormat::Float16 => {
                for chunk in data_to_parse.as_chunks::<2>().0 {
                    let val_raw = f16::from_le_bytes([chunk[0], chunk[1]]);
                    samples.push(Complex32::new(val_raw.to_f32(), 0.0));
                }
            }
            StreamFormat::Float32 => {
                for chunk in data_to_parse.as_chunks::<4>().0 {
                    let val = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    samples.push(Complex32::new(val, 0.0));
                }
            }
            StreamFormat::Json => {
                return Err(Error::Protocol(
                    "Binary spectrum parsing not applicable for JSON format".to_string(),
                ));
            }
        }

        Ok(samples)
    }

    fn parse_histogram_samples(
        &self,
        metadata: &PacketMetadata,
        data: &[u8],
    ) -> Result<Vec<Complex32>> {
        // Similar to spectrum but with 2D structure flattened to 1D
        self.parse_spectrum_samples(metadata, data)
    }

    fn parse_category_samples(
        &self,
        metadata: &PacketMetadata,
        data: &[u8],
    ) -> Result<Vec<Complex32>> {
        // Category data is single values per category
        self.parse_spectrum_samples(metadata, data)
    }

    fn parse_json_samples(
        &self,
        metadata: &PacketMetadata,
        json: &serde_json::Value,
    ) -> Result<Vec<Complex32>> {
        let samples_array = json["samples"]
            .as_array()
            .ok_or_else(|| Error::Protocol("Missing samples array in JSON".to_string()))?;

        match metadata.payload {
            PayloadType::Iq => {
                // IQ samples are flat array of alternating I and Q values
                let mut samples = Vec::new();
                let values: Result<Vec<f32>, _> = samples_array
                    .iter()
                    .map(|v| {
                        v.as_f64()
                            .ok_or_else(|| Error::Protocol("Invalid sample value".to_string()))
                            .map(|f| f as f32)
                    })
                    .collect();

                let values = values?;
                for chunk in values.as_chunks::<2>().0 {
                    samples.push(Complex32::new(chunk[0], chunk[1]));
                }
                Ok(samples)
            }
            PayloadType::Spectra => {
                // Spectrum samples can be 2D array
                let mut samples = Vec::new();
                for spectrum in samples_array {
                    if let Some(spectrum_array) = spectrum.as_array() {
                        for value in spectrum_array {
                            let val = value.as_f64().ok_or_else(|| {
                                Error::Protocol("Invalid spectrum value".to_string())
                            })? as f32;
                            samples.push(Complex32::new(val, 0.0));
                        }
                    }
                }
                Ok(samples)
            }
            PayloadType::Histogram | PayloadType::Categories => {
                // Single dimension arrays
                let mut samples = Vec::new();
                for value in samples_array {
                    let val = value
                        .as_f64()
                        .ok_or_else(|| Error::Protocol("Invalid value".to_string()))?
                        as f32;
                    samples.push(Complex32::new(val, 0.0));
                }
                Ok(samples)
            }
        }
    }

    /// Decompress spectrum data using RTSA-style decompression algorithms
    #[cfg(feature = "file")]
    fn decompress_spectrum_data(
        &self,
        compressed_data: &[u8],
        compression_factor: u32,
        metadata: &PacketMetadata,
    ) -> Result<Vec<u8>> {
        use crate::decompression::Decompressor;
        use tracing::debug;

        debug!(
            "Starting RTSA-style spectrum decompression: {} bytes, compression factor {}, sample_size {}",
            compressed_data.len(),
            compression_factor,
            metadata.sample_size
        );

        // Calculate expected dimensions for wavelet transform
        let expected_samples = metadata.sample_size as usize;
        let sample_depth = metadata.sample_depth.unwrap_or(1) as usize;

        // For spectrum data, we typically have a 2D matrix that gets flattened
        // Try to determine reasonable dimensions for the wavelet transform
        let (num_rows, num_cols) =
            self.calculate_wavelet_dimensions(expected_samples, sample_depth);

        debug!(
            "Using wavelet dimensions: {} rows × {} cols for {} expected samples",
            num_rows, num_cols, expected_samples
        );

        // Use the existing RTSA decompression infrastructure
        let decompressor = Decompressor::new();
        let decompressed_f32 =
            decompressor.decompress(compressed_data, compression_factor, num_rows, num_cols)?;

        let encode_scale = self.int16_encode_scale(metadata);
        let result_bytes = self.convert_f32_to_target_format(&decompressed_f32, encode_scale)?;

        debug!(
            "Successfully decompressed using RTSA algorithms: {} -> {} bytes",
            compressed_data.len(),
            result_bytes.len()
        );

        Ok(result_bytes)
    }

    /// Decompress spectrum data using RTSA-style decompression algorithms
    #[cfg(not(feature = "file"))]
    fn decompress_spectrum_data(
        &self,
        _compressed_data: &[u8],
        _compression_factor: u32,
        _metadata: &PacketMetadata,
    ) -> Result<Vec<u8>> {
        Err(Error::Config(
            "The 'file' feature is required for RTSA spectrum decompression".to_string(),
        ))
    }

    /// Calculate optimal dimensions for wavelet transform based on sample data
    #[cfg(feature = "file")]
    fn calculate_wavelet_dimensions(
        &self,
        expected_samples: usize,
        sample_depth: usize,
    ) -> (usize, usize) {
        // For spectrum data, we need dimensions that are powers of 2 for optimal wavelet transform
        let total_elements = expected_samples * sample_depth;

        // Find the largest power of 2 that fits in our data
        let sqrt_elements = (total_elements as f64).sqrt() as usize;
        let mut dim = 1;
        while dim <= sqrt_elements {
            dim <<= 1;
        }
        dim >>= 1; // Back off to largest power of 2 <= sqrt

        // Ensure we have valid dimensions
        let num_rows = std::cmp::max(dim, 1);
        let num_cols = if total_elements > 0 {
            std::cmp::max(total_elements / num_rows, 1)
        } else {
            1
        };

        (num_rows, num_cols)
    }

    /// Convert f32 decompressed data back to the target format.
    /// `encode_scale` is the wire's int16 encode multiplier
    /// (`int16 = round(value * encode_scale)`), already validated by
    /// [`Self::int16_encode_scale`].
    #[cfg(feature = "file")]
    fn convert_f32_to_target_format(&self, f32_data: &[f32], encode_scale: f64) -> Result<Vec<u8>> {
        let mut result = Vec::new();

        match self.format {
            StreamFormat::Int16 => {
                let scale = encode_scale as f32;
                for &value in f32_data {
                    let int_val = (value * scale)
                        .round()
                        .clamp(i16::MIN as f32, i16::MAX as f32)
                        as i16;
                    result.extend_from_slice(&int_val.to_le_bytes());
                }
            }
            StreamFormat::Float16 => {
                // Convert f32 to f16
                for &value in f32_data {
                    let f16_val = f16::from_f32(value);
                    result.extend_from_slice(&f16_val.to_le_bytes());
                }
            }
            StreamFormat::Float32 => {
                // Direct f32 conversion
                for &value in f32_data {
                    result.extend_from_slice(&value.to_le_bytes());
                }
            }
            StreamFormat::Json => {
                return Err(Error::Protocol("Cannot convert to JSON format".to_string()));
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {

    /// Framing throughput: the full `process_data` path — buffer append,
    /// JSON metadata parse, separator scan, payload split — fed realistic
    /// int16 packets in network-sized chunks. Run with
    /// `cargo test --release --all-features -- --ignored framing_throughput --nocapture`.
    #[test]
    #[ignore = "throughput meter, run with --release and --nocapture"]
    fn framing_throughput_meter() {
        let n = 49_152usize;
        // One wire packet: JSON metadata, RS, payload, newline+RS framing
        // as the server sends it.
        let header = format!(
            "{{\"startTime\":1786000000.0,\"endTime\":1786000000.0008,\"startFrequency\":821180250.0,\"endFrequency\":882132250.0,\"sampleFrequency\":61440000.0,\"payload\":\"iq\",\"unit\":\"generic\",\"minPower\":-2,\"maxPower\":2,\"sampleSize\":2,\"sampleDepth\":1,\"scale\":1000000.0,\"samples\":{n}}}"
        );
        let payload: Vec<u8> = (0..n * 4).map(|i| (i * 31 % 251) as u8).collect();
        let mut wire = Vec::new();
        wire.extend_from_slice(header.as_bytes());
        wire.push(0x1e);
        wire.extend_from_slice(&payload);
        wire.push(b'\n');
        // ~24 packets, split into 157 KiB chunks like reqwest delivers.
        let mut stream_bytes = Vec::new();
        for _ in 0..24 {
            stream_bytes.extend_from_slice(&wire);
        }
        let chunks: Vec<Bytes> = stream_bytes
            .chunks(157 * 1024)
            .map(Bytes::copy_from_slice)
            .collect();

        let mut parser = StreamParser::new(StreamFormat::Int16, Some(1e6)).unwrap();
        // Warm up.
        for c in &chunks {
            let _ = parser.process_data(c).unwrap();
        }
        let iters = 60;
        let t0 = std::time::Instant::now();
        let mut samples = 0usize;
        for _ in 0..iters {
            for c in &chunks {
                for p in parser.process_data(c).unwrap() {
                    samples += p.samples.len();
                }
            }
        }
        let dt = t0.elapsed().as_secs_f64();
        println!(
            "framing+decode int16   {:8.1} MS/s   {:7.1} MB/s",
            samples as f64 / dt / 1e6,
            (iters * stream_bytes.len()) as f64 / dt / 1e6
        );
    }

    /// Not a correctness test: a decode-throughput meter for the three IQ
    /// wire formats, run explicitly with
    /// `cargo test --release --all-features -- --ignored decode_throughput --nocapture`.
    /// It exists because the transport ceiling is measurable with `curl`,
    /// and whether the parser can stay above it decides where optimisation
    /// effort goes.
    #[test]
    #[ignore = "throughput meter, run with --release and --nocapture"]
    fn decode_throughput_meter() {
        let parser_i16 = StreamParser::new(StreamFormat::Int16, Some(1e6)).unwrap();
        let parser_f16 = StreamParser::new(StreamFormat::Float16, None).unwrap();
        let parser_f32 = StreamParser::new(StreamFormat::Float32, None).unwrap();
        let bench_meta = |scale: Option<f64>| PacketMetadata {
            start_time: 0.0,
            end_time: 0.001,
            start_time_day: None,
            end_time_day: None,
            start_frequency: 0.0,
            end_frequency: 1e6,
            sample_frequency: Some(61.44e6),
            samples: 49_152,
            unit: "generic".to_string(),
            payload: PayloadType::Iq,
            min_power: -100,
            max_power: 0,
            sample_depth: None,
            sample_size: 2,
            scale,
            antenna: None,
            categories: None,
            compression: None,
        };
        let meta_i16 = bench_meta(Some(1e6));
        let meta_plain = bench_meta(None);

        // 49_152 samples, the packet size a V6 sends at full span.
        let n = 49_152usize;
        let payload_4: Vec<u8> = (0..n * 4).map(|i| (i * 31 % 251) as u8).collect();
        let payload_8: Vec<u8> = (0..n * 8).map(|i| (i * 31 % 251) as u8).collect();

        let run = |name: &str, f: &mut dyn FnMut() -> usize, bytes_per_iter: usize| {
            // Warm up, then time enough iterations for a stable figure.
            for _ in 0..20 {
                f();
            }
            let iters = 400;
            let t0 = std::time::Instant::now();
            let mut total = 0usize;
            for _ in 0..iters {
                total += f();
            }
            let dt = t0.elapsed().as_secs_f64();
            println!(
                "{name:<22} {:8.1} MS/s   {:7.1} MB/s",
                total as f64 / dt / 1e6,
                (iters * bytes_per_iter) as f64 / dt / 1e6
            );
        };

        run(
            "int16 decode",
            &mut || {
                parser_i16
                    .parse_iq_samples(&meta_i16, &payload_4)
                    .unwrap()
                    .len()
            },
            payload_4.len(),
        );
        run(
            "float16 decode",
            &mut || {
                parser_f16
                    .parse_iq_samples(&meta_plain, &payload_4)
                    .unwrap()
                    .len()
            },
            payload_4.len(),
        );
        run(
            "float32 decode",
            &mut || {
                parser_f32
                    .parse_iq_samples(&meta_plain, &payload_8)
                    .unwrap()
                    .len()
            },
            payload_8.len(),
        );
    }
    use super::*;

    /// A minimal int16 IQ header for `n` pairs, without its terminator.
    fn scan_header(n: usize) -> Vec<u8> {
        format!(
            r#"{{"startTime":0.0,"endTime":0.001,"startFrequency":95e6,"endFrequency":105e6,"sampleFrequency":15360000.0,"samples":{n},"unit":"volt","payload":"iq","minPower":-120,"maxPower":0,"sampleSize":2}}"#
        )
        .into_bytes()
    }

    /// The scan's framing contract on the hardware-verified wire format:
    /// the header's text, both separator bytes spent, and the payload
    /// starting right after them. A lone terminator of either kind is
    /// still accepted, and consumes exactly one byte — a payload whose
    /// first byte is `0x1E` must not be eaten as a second separator.
    #[test]
    fn the_scan_frames_the_two_byte_separator_and_lone_terminators() {
        let header = scan_header(2);
        let mut wire = header.clone();
        wire.extend_from_slice(&[ASCII_LINE_FEED, ASCII_RECORD_SEPARATOR, 1, 2, 3]);
        let HeaderScan::Header {
            metadata,
            json,
            payload_start,
            rejected,
        } = scan_packet_header(&wire)
        else {
            panic!("a complete header must be found");
        };
        assert_eq!(metadata.samples, 2);
        assert_eq!(&wire[json], &header[..]);
        assert_eq!(payload_start, header.len() + 2);
        assert_eq!(rejected, 0);

        for lone in [ASCII_LINE_FEED, ASCII_RECORD_SEPARATOR] {
            let mut wire = header.clone();
            wire.extend_from_slice(&[lone, ASCII_RECORD_SEPARATOR, 0x42]);
            let HeaderScan::Header { payload_start, .. } = scan_packet_header(&wire) else {
                panic!("a complete header must be found");
            };
            // LF+RS is the two-byte separator; RS+RS is a lone RS and a
            // payload that happens to start with 0x1E.
            let expected = if lone == ASCII_LINE_FEED {
                header.len() + 2
            } else {
                header.len() + 1
            };
            assert_eq!(payload_start, expected, "lone terminator {lone:#04x}");
        }
    }

    /// A stray `{` from binary payload ahead of a real header shares the
    /// header's terminator. The resync must step one byte past the stray
    /// brace, not past the terminator — the coarser skip jumps straight
    /// over the header.
    #[test]
    fn a_stray_brace_sharing_the_terminator_does_not_hide_the_header() {
        let header = scan_header(4);
        let mut wire = vec![0x00, b'{', 0x7F, b'{', 0x01];
        let header_at = wire.len();
        wire.extend_from_slice(&header);
        wire.extend_from_slice(&[ASCII_LINE_FEED, ASCII_RECORD_SEPARATOR]);
        let HeaderScan::Header { json, rejected, .. } = scan_packet_header(&wire) else {
            panic!("the real header must be found");
        };
        assert_eq!(json, header_at..header_at + header.len());
        assert_eq!(rejected, 2, "both stray braces were tried and rejected");
    }

    /// Regions with no header in them are spent whole, and an
    /// unterminated candidate is kept while everything before it goes.
    #[test]
    fn the_scan_spends_headerless_regions_and_keeps_an_unfinished_candidate() {
        let mut wire = b"{not a header}\n{nor this}\x1e".to_vec();
        let junk_len = wire.len();
        wire.extend_from_slice(&scan_header(1));
        wire.push(ASCII_RECORD_SEPARATOR);
        let HeaderScan::Header { json, rejected, .. } = scan_packet_header(&wire) else {
            panic!("the header after the junk must be found");
        };
        assert_eq!(json.start, junk_len);
        assert_eq!(rejected, 2);

        // The candidate at index 7 has no terminator yet: kept, with the
        // junk region and the garbage before it spent.
        let partial = b"{junk}\n\x00{\"startTime\":0.0";
        let HeaderScan::Incomplete {
            skippable,
            rejected,
        } = scan_packet_header(partial)
        else {
            panic!("an unterminated header is not complete");
        };
        assert_eq!(skippable, 8);
        assert_eq!(rejected, 1);

        // No candidate at all: everything is spent.
        let HeaderScan::Incomplete {
            skippable,
            rejected,
        } = scan_packet_header(b"no brace here")
        else {
            panic!("no header without a brace");
        };
        assert_eq!(skippable, 13);
        assert_eq!(rejected, 0);
        assert!(matches!(
            scan_packet_header(b""),
            HeaderScan::Incomplete {
                skippable: 0,
                rejected: 0
            }
        ));
    }

    /// Brace-dense garbage without a terminator is the shape that made a
    /// per-candidate terminator search quadratic; every candidate here
    /// shares one terminator and is tried exactly once.
    #[test]
    fn brace_dense_garbage_is_tried_once_per_candidate() {
        let braces = 4096;
        let mut wire = vec![b'{'; braces];
        wire.push(ASCII_LINE_FEED);
        let header_at = wire.len();
        wire.extend_from_slice(&scan_header(1));
        wire.extend_from_slice(&[ASCII_LINE_FEED, ASCII_RECORD_SEPARATOR]);
        let HeaderScan::Header { json, rejected, .. } = scan_packet_header(&wire) else {
            panic!("the header after the garbage must be found");
        };
        assert_eq!(json.start, header_at);
        assert_eq!(rejected, braces);
    }

    fn iq_packet_bytes(samples: &[i16], scale: Option<f64>) -> Vec<u8> {
        let pairs = samples.len() / 2;
        let mut json = format!(
            r#"{{"startTime":0.0,"endTime":1.0,"startFrequency":0.0,"endFrequency":1.0,"samples":{},"unit":"generic","payload":"iq","minPower":-100,"maxPower":0,"sampleSize":2"#,
            pairs
        );
        if let Some(s) = scale {
            json.push_str(&format!(r#","scale":{}"#, s));
        }
        json.push('}');

        let mut out = json.into_bytes();
        out.push(ASCII_RECORD_SEPARATOR);
        for &v in samples {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    #[test]
    fn test_streaming_performance_stats_default_zero() {
        let stats = StreamingPerformanceStats::default();
        assert_eq!(stats.packets_parsed, 0);
        assert_eq!(stats.bytes_processed, 0);
        assert_eq!(stats.samples_decoded, 0);
        assert_eq!(stats.parse_errors, 0);
    }

    /// Build a minimal `PacketMetadata` JSON literal with a custom
    /// `samples` value. Used by the deserializer tests below.
    fn packet_metadata_json(samples_field: &str) -> String {
        format!(
            r#"{{"startTime":0.0,"endTime":1.0,"startFrequency":0.0,"endFrequency":1.0,"samples":{samples_field},"unit":"volt","payload":"iq","minPower":0,"maxPower":1,"sampleSize":2}}"#
        )
    }

    #[test]
    fn samples_field_deserializes_from_count() {
        let json = packet_metadata_json("1024");
        let m: PacketMetadata = serde_json::from_str(&json).expect("count form must parse");
        assert_eq!(m.samples, 1024);
    }

    #[test]
    fn samples_field_deserializes_from_array() {
        // Four floats -> array length 4. parse_json_chunk would later
        // overwrite this with the decoded Complex32 count (2 IQ pairs),
        // but the deserializer alone just captures the raw length.
        let json = packet_metadata_json("[1.5, -2.5, 3.5, -4.5]");
        let m: PacketMetadata = serde_json::from_str(&json).expect("array form must parse");
        assert_eq!(m.samples, 4);
    }

    #[test]
    fn samples_field_rejects_unsupported_shapes() {
        for bad in ["true", "\"abc\"", "null", "{\"x\":1}"] {
            let json = packet_metadata_json(bad);
            assert!(
                serde_json::from_str::<PacketMetadata>(&json).is_err(),
                "expected serde error for samples={bad}"
            );
        }
    }

    #[test]
    fn samples_field_rejects_negative_count() {
        let json = packet_metadata_json("-1");
        assert!(serde_json::from_str::<PacketMetadata>(&json).is_err());
    }

    /// The per-packet `scale` is the server's encode multiplier
    /// (`int16 = value * scale`), so decoding divides. Verified on live
    /// hardware: a dBm spectra stream with `"scale":100` carries raw
    /// values like `-11378` = −113.78 dBm. The earlier decoder
    /// multiplied instead — wrong by a factor of scale².
    #[test]
    fn test_int16_uses_metadata_scale() {
        let scale = 16384.0_f64;
        let payload = iq_packet_bytes(&[16384, -16384, 32767, -32768], Some(scale));
        let mut parser = StreamParser::new(StreamFormat::Int16, None).unwrap();
        let packets = parser.process_data(&Bytes::from(payload)).unwrap();
        assert_eq!(packets.len(), 1);
        let s = &packets[0].samples;
        assert_eq!(s.len(), 2);
        assert!((s[0].re - 1.0).abs() < 1e-6, "16384 / 16384 = 1.0");
        assert!((s[0].im - -1.0).abs() < 1e-6);
        assert!((s[1].re - (32767.0 / scale as f32)).abs() < 1e-6);

        let stats = parser.stats();
        assert_eq!(stats.packets_parsed, 1);
        assert_eq!(stats.samples_decoded, 2);
        assert!(stats.parse_errors == 0);
    }

    #[test]
    fn test_int16_default_scale_when_metadata_missing() {
        let payload = iq_packet_bytes(&[16384, -16384], None);
        let mut parser = StreamParser::new(StreamFormat::Int16, None).unwrap();
        let packets = parser.process_data(&Bytes::from(payload)).unwrap();
        assert_eq!(packets.len(), 1);
        let s = &packets[0].samples[0];
        assert!((s.re - 0.5).abs() < 1e-6);
        assert!((s.im - -0.5).abs() < 1e-6);
    }

    /// The constructor `scale` is the `?scale=N` requested from the
    /// server — also an encode multiplier, so decoding divides by it.
    #[test]
    fn test_int16_constructor_scale_used_when_metadata_missing() {
        let constructor_scale = 1000.0_f64;
        let payload = iq_packet_bytes(&[1000, -1000], None);
        let mut parser = StreamParser::new(StreamFormat::Int16, Some(constructor_scale)).unwrap();
        let packets = parser.process_data(&Bytes::from(payload)).unwrap();
        let s = &packets[0].samples[0];
        assert!((s.re - 1.0).abs() < 1e-6, "1000 / 1000 = 1.0");
        assert!((s.im - -1.0).abs() < 1e-6);
    }

    /// Real Aaronia servers separate the JSON header from the binary
    /// payload with TWO bytes — `{json}\n\x1e<binary>` — verified on live
    /// SpectranV6 hardware across formats and payload types. Consuming
    /// only the LF shifted every binary sample by one byte (garbage
    /// amplitudes that still "parsed" cleanly).
    #[test]
    fn test_lf_rs_two_byte_separator() {
        let header = r#"{"startTime":0.0,"endTime":1.0,"startFrequency":0.0,"endFrequency":1.0,"samples":2,"unit":"volt","payload":"iq","minPower":0,"maxPower":1,"sampleSize":2}"#;
        let mut payload = header.as_bytes().to_vec();
        payload.push(ASCII_LINE_FEED);
        payload.push(ASCII_RECORD_SEPARATOR);
        for v in [0.25f32, -0.25, 0.5, -0.5] {
            payload.extend_from_slice(&v.to_le_bytes());
        }

        let mut parser = StreamParser::new(StreamFormat::Float32, None).unwrap();
        let packets = parser.process_data(&Bytes::from(payload)).unwrap();
        assert_eq!(packets.len(), 1);
        let s = &packets[0].samples;
        assert_eq!(s.len(), 2);
        assert!(
            (s[0].re - 0.25).abs() < 1e-6,
            "first float must not be byte-shifted by the RS: got {}",
            s[0].re
        );
        assert!((s[1].im - -0.5).abs() < 1e-6);
        assert_eq!(parser.stats().parse_errors, 0);
    }

    /// Chunk boundary between the LF and the RS must not cause a
    /// mis-parse: nothing is consumed until the full payload arrives, so
    /// the re-parse after the next chunk sees both separator bytes.
    #[test]
    fn test_lf_rs_separator_split_across_chunks() {
        let header = r#"{"startTime":0.0,"endTime":1.0,"startFrequency":0.0,"endFrequency":1.0,"samples":1,"unit":"volt","payload":"iq","minPower":0,"maxPower":1,"sampleSize":2}"#;
        let mut chunk1 = header.as_bytes().to_vec();
        chunk1.push(ASCII_LINE_FEED);
        let mut chunk2 = vec![ASCII_RECORD_SEPARATOR];
        for v in [0.75f32, -0.75] {
            chunk2.extend_from_slice(&v.to_le_bytes());
        }

        let mut parser = StreamParser::new(StreamFormat::Float32, None).unwrap();
        assert!(
            parser
                .process_data(&Bytes::from(chunk1))
                .unwrap()
                .is_empty(),
            "no packet before the payload arrives"
        );
        let packets = parser.process_data(&Bytes::from(chunk2)).unwrap();
        assert_eq!(packets.len(), 1);
        assert!((packets[0].samples[0].re - 0.75).abs() < 1e-6);
    }

    /// Spectra packets hold `samples` frames × `sampleSize` bins. Live
    /// hardware sends e.g. samples=64, sampleSize=820 with exactly
    /// 64·820·4 payload bytes; the earlier size formula ignored the
    /// frame count and truncated every multi-frame packet.
    #[test]
    fn test_spectra_multi_frame_payload_size() {
        let header = r#"{"startTime":0.0,"endTime":1.0,"startFrequency":100.0,"endFrequency":200.0,"samples":3,"unit":"dBm","payload":"spectra","minPower":-120,"maxPower":-20,"sampleSize":4,"sampleDepth":1}"#;
        let mut payload = header.as_bytes().to_vec();
        payload.push(ASCII_LINE_FEED);
        payload.push(ASCII_RECORD_SEPARATOR);
        // 3 frames × 4 bins of float32.
        for i in 0..12 {
            payload.extend_from_slice(&(-100.0f32 - i as f32).to_le_bytes());
        }
        // Follow with a second packet to prove the first consumed exactly
        // the right number of bytes (no over/under-read desync).
        let header2 = r#"{"startTime":1.0,"endTime":2.0,"startFrequency":100.0,"endFrequency":200.0,"samples":1,"unit":"dBm","payload":"spectra","minPower":-120,"maxPower":-20,"sampleSize":4,"sampleDepth":1}"#;
        payload.extend_from_slice(header2.as_bytes());
        payload.push(ASCII_LINE_FEED);
        payload.push(ASCII_RECORD_SEPARATOR);
        for i in 0..4 {
            payload.extend_from_slice(&(-50.0f32 - i as f32).to_le_bytes());
        }

        let mut parser = StreamParser::new(StreamFormat::Float32, None).unwrap();
        let packets = parser.process_data(&Bytes::from(payload)).unwrap();
        assert_eq!(packets.len(), 2, "both packets must decode");
        assert_eq!(packets[0].samples.len(), 12, "3 frames × 4 bins");
        assert!((packets[0].samples[0].re - -100.0).abs() < 1e-6);
        assert!((packets[0].samples[11].re - -111.0).abs() < 1e-6);
        assert_eq!(packets[1].samples.len(), 4);
        assert!((packets[1].samples[0].re - -50.0).abs() < 1e-6);
        assert_eq!(parser.stats().parse_errors, 0);
    }

    /// Regression from live hardware (SpectranV6 HTTP-server mission):
    /// real packet headers carry `"antenna":{"name":""}` with no position
    /// fields. When `AntennaSpec` required lat/lon/azimuth/declination,
    /// metadata deserialization failed on *every* packet and the parser
    /// resynced endlessly — zero packets ever decoded from a live device.
    /// The header below is byte-for-byte from a real capture (binary
    /// payload shortened).
    #[test]
    fn test_metadata_accepts_partial_antenna_from_real_device() {
        let header = r#"{"startTime":1783032873.309269,"endTime":1783032873.310336,"startTimeDay":82473.309268706,"endTimeDay":82473.310335372,"startFrequency":175423838.71,"endFrequency":224575838.71,"sampleFrequency":61439951.213,"minPower":-2,"maxPower":2,"sampleSize":2,"sampleDepth":1,"payload":"iq","unit":"volt","antenna":{"name":""},"scale":16384,"samples":2}"#;
        let mut payload = header.as_bytes().to_vec();
        payload.push(ASCII_LINE_FEED);
        for v in [0.25f32, -0.25, 0.5, -0.5] {
            payload.extend_from_slice(&v.to_le_bytes());
        }

        let mut parser = StreamParser::new(StreamFormat::Float32, None).unwrap();
        let packets = parser.process_data(&Bytes::from(payload)).unwrap();
        assert_eq!(
            packets.len(),
            1,
            "real-device header with partial antenna must decode"
        );
        assert_eq!(packets[0].samples.len(), 2);
        let antenna = packets[0].metadata.antenna.as_ref().unwrap();
        assert_eq!(antenna.name, "");
        assert_eq!(
            antenna.latitude, None,
            "unreported position must be None, not a fake 0.0 coordinate"
        );
        assert_eq!(parser.stats().parse_errors, 0);
    }

    #[test]
    fn test_lf_separator_accepted() {
        let payload = {
            let mut p = iq_packet_bytes(&[1000, -1000], None);
            // Replace the RS with a LF to confirm we accept the
            // "line feed (ASCII 10)" separator.
            for b in p.iter_mut() {
                if *b == ASCII_RECORD_SEPARATOR {
                    *b = ASCII_LINE_FEED;
                    break;
                }
            }
            p
        };
        let mut parser = StreamParser::new(StreamFormat::Int16, None).unwrap();
        let packets = parser.process_data(&Bytes::from(payload)).unwrap();
        assert_eq!(packets.len(), 1);
    }

    #[test]
    fn test_stream_format_from_str() {
        assert_eq!(StreamFormat::from_str("json").unwrap(), StreamFormat::Json);
        assert_eq!(
            StreamFormat::from_str("int16").unwrap(),
            StreamFormat::Int16
        );
        assert_eq!(
            StreamFormat::from_str("float16").unwrap(),
            StreamFormat::Float16
        );
        assert_eq!(
            StreamFormat::from_str("float32").unwrap(),
            StreamFormat::Float32
        );

        assert!(StreamFormat::from_str("invalid").is_err());
    }

    #[test]
    fn test_stream_format_as_str() {
        assert_eq!(StreamFormat::Json.as_str(), "json");
        assert_eq!(StreamFormat::Int16.as_str(), "int16");
        assert_eq!(StreamFormat::Float16.as_str(), "float16");
        assert_eq!(StreamFormat::Float32.as_str(), "float32");
    }

    #[test]
    fn test_stream_parser_creation() {
        let parser = StreamParser::new(StreamFormat::Json, None);
        assert!(parser.is_ok());

        let parser = StreamParser::new(StreamFormat::Int16, Some(0.1));
        assert!(parser.is_ok());
    }

    #[test]
    fn test_stream_parser_empty_data() {
        let mut parser = StreamParser::new(StreamFormat::Json, None).unwrap();
        let empty_data = Bytes::from(vec![]);

        let result = parser.process_data(&empty_data);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "the half crate's f16 conversion uses hardware intrinsics / inline \
                  assembly that Miri cannot interpret"
    )]
    fn test_binary_sample_scale_factors() {
        // Test int16 scale factor calculation
        let scale_factor = 1.0 / 32768.0;
        let test_val = 16384i16;
        let expected = test_val as f32 * scale_factor;
        assert!((expected - 0.5).abs() < 1e-6);

        // Test float16 to float32 conversion
        let f16_val = f16::from_f32(1.5);
        let f32_val = f16_val.to_f32();
        assert!((f32_val - 1.5).abs() < 1e-3);
    }

    #[test]
    fn test_iq_sample_pairing() {
        // Test that IQ samples are properly paired (I,Q,I,Q,...)
        let test_data = [1.0f32, 2.0f32, 3.0f32, 4.0f32];
        let iq_pairs: Vec<Complex32> = test_data
            .chunks(2)
            .map(|pair| Complex32::new(pair[0], pair[1]))
            .collect();

        assert_eq!(iq_pairs.len(), 2);
        assert_eq!(iq_pairs[0], Complex32::new(1.0, 2.0));
        assert_eq!(iq_pairs[1], Complex32::new(3.0, 4.0));
    }

    #[test]
    fn test_ascii_record_separator_constant() {
        // Test that ASCII Record Separator is correctly defined
        assert_eq!(ASCII_RECORD_SEPARATOR, 30);
        assert_eq!(ASCII_RECORD_SEPARATOR, 0x1E);
    }

    #[test]
    fn test_default_int16_scale_constant() {
        // Decoding with the fallback divides by full scale.
        assert!((1.0 / DEFAULT_INT16_ENCODE_SCALE - (1.0 / 32768.0)).abs() < 1e-10);
    }

    fn iq_packet(start_time: f64, end_time: f64) -> StreamPacket {
        let metadata = PacketMetadata {
            start_time,
            end_time,
            start_time_day: None,
            end_time_day: None,
            start_frequency: 0.0,
            end_frequency: 1e6,
            sample_frequency: Some(1e6),
            samples: 1,
            unit: "generic".to_string(),
            payload: PayloadType::Iq,
            min_power: -100,
            max_power: 0,
            sample_depth: None,
            sample_size: 2,
            scale: None,
            antenna: None,
            categories: None,
            compression: None,
        };
        StreamPacket::new(metadata, vec![Complex32::new(0.0, 0.0)])
    }

    #[test]
    fn drop_detector_first_packet_is_continuous() {
        let mut det = DropDetector::default();
        let p = iq_packet(100.0, 100.001);
        assert_eq!(det.observe(&p), DropResult::Continuous);
        assert_eq!(det.drops(), 0);
    }

    #[test]
    fn drop_detector_continuous_packets_have_no_drop() {
        let mut det = DropDetector::default();
        det.observe(&iq_packet(100.000, 100.001));
        let result = det.observe(&iq_packet(100.001, 100.002));
        assert_eq!(result, DropResult::Continuous);
        assert_eq!(det.drops(), 0);
    }

    #[test]
    fn drop_detector_flags_gap_above_tolerance() {
        let mut det = DropDetector::new(1e-3);
        det.observe(&iq_packet(100.000, 100.001));
        let result = det.observe(&iq_packet(100.100, 100.101));
        match result {
            DropResult::Drop { gap_seconds } => {
                assert!((gap_seconds - 0.099).abs() < 1e-6, "gap = {}", gap_seconds);
            }
            _ => panic!("expected Drop, got {:?}", result),
        }
        assert_eq!(det.drops(), 1);
        assert!((det.cumulative_gap_seconds() - 0.099).abs() < 1e-6);
    }

    #[test]
    fn drop_detector_reset_clears_state() {
        let mut det = DropDetector::default();
        det.observe(&iq_packet(100.0, 100.001));
        det.observe(&iq_packet(100.5, 100.501)); // drop
        assert_eq!(det.drops(), 1);
        det.reset();
        assert_eq!(det.drops(), 0);
        assert_eq!(det.cumulative_gap_seconds(), 0.0);
        // After reset, the next observation must be Continuous (no prior).
        assert_eq!(
            det.observe(&iq_packet(200.0, 200.001)),
            DropResult::Continuous
        );
    }
}

// End of streaming_formats.rs
