//! HTTP Endpoints Implementation
//!
//! Complete implementation of RTSA HTTP endpoints based on specification:
//! - /sample, /samples - Single and multi-sample retrieval
//! - /stream - Real-time streaming with multiple formats
//! - /inputs - Input enumeration and dynamic creation
//! - /control - Device control and configuration
//! - /remoteconfig - Advanced configuration management (requires separate license)
//! - /healthstatus - Device health monitoring
//! - /info - Server information
//! - /user - Authentication and user management

use crate::http_streaming::PacketMetadata;
use crate::{Error, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, warn};

/// Defines the authentication method to be used when connecting to the RTSA device.
#[derive(Clone)]
pub enum AuthMethod {
    /// HTTP Basic Authentication using a username and password.
    Basic {
        /// Username
        username: String,
        /// Password
        password: String,
    },
    /// Token-based authentication using an `RToken`.
    ///
    /// The token can be obtained from the `/user` endpoint.
    Token {
        /// Token
        token: String,
    },
    /// No authentication.
    None,
}

impl AuthMethod {
    /// Apply this authentication method to a request.
    ///
    /// The one definition of the RTSA auth headers — in particular the
    /// `RToken` scheme string, which is not standard HTTP. The endpoints
    /// client, the streaming source and the link-budget probe all route
    /// through here, so a future change to the scheme cannot leave one
    /// of them authenticating the old way.
    pub fn apply_to(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            AuthMethod::Basic { username, password } => {
                builder.basic_auth(username, Some(password))
            }
            AuthMethod::Token { token } => {
                builder.header("Authorization", format!("RToken {token}"))
            }
            AuthMethod::None => builder,
        }
    }
}

impl std::fmt::Debug for AuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthMethod::None => write!(f, "AuthMethod::None"),
            AuthMethod::Basic { username, .. } => write!(
                f,
                "AuthMethod::Basic {{ username: {:?}, password: [REDACTED] }}",
                username
            ),
            AuthMethod::Token { .. } => write!(f, "AuthMethod::Token {{ token: [REDACTED] }}"),
        }
    }
}

/// The `reqwest` client settings every RTSA connection shares, with the
/// connect timeout as the one per-caller knob.
///
/// One definition rather than three: the endpoints client, the
/// `HttpSource` streaming client and the link-budget probe each used to
/// hand-roll their own builder with a different subset of these
/// settings, so an "RTSA-compatible" fix landed in one and silently
/// missed the others.
///
/// Deliberately **no** client-level `.timeout(...)`: it would apply to
/// the whole response body and terminate a long-lived `/stream`
/// connection mid-flight (this bit us at 120 s). Control-plane calls add
/// a per-request timeout instead.
pub(crate) fn rtsa_client_builder(connect_timeout: std::time::Duration) -> reqwest::ClientBuilder {
    Client::builder()
        .connect_timeout(connect_timeout)
        .user_agent(crate::utils::user_agent())
        // Aggressive TCP optimizations for maximum throughput
        .tcp_keepalive(std::time::Duration::from_secs(30)) // More frequent keepalives
        .tcp_nodelay(true) // Disable Nagle algorithm
        // Optimized connection pooling for RTSA: control + streaming channels
        .pool_idle_timeout(std::time::Duration::from_secs(300)) // Keep connections during long streams
        .pool_max_idle_per_host(2) // Control channel + streaming channel
        // Title-case headers on the HTTP/1.1 path, for RTSA compatibility.
        //
        // No `.http1_only()`: RTSA itself speaks only HTTP/1.1, but over
        // plain `http://` that is what reqwest uses anyway (no ALPN, no
        // prior-knowledge h2), so forcing it protects nothing — while over
        // `https://` it breaks streaming through a TLS-terminating proxy
        // whose ALPN offers only h2. Let ALPN negotiate; a direct RTSA TLS
        // endpoint still lands on HTTP/1.1.
        .http1_title_case_headers()
        // And when ALPN does land on h2, do not let its flow control be
        // the link: hyper's default 2 MiB stream window caps a stream at
        // window / RTT (~84 MB/s at 25 ms, under the 123 MB/s a 30.72 MS/s
        // int16 stream needs), which the link-budget check would then
        // measure and blame on the path. The adaptive window sizes itself
        // to the bandwidth-delay product instead.
        .http2_adaptive_window(true)
}

/// Validate a base URL before any RTSA connection is opened: parseable,
/// and an `http`/`https` scheme — a client that followed a `file://` URL
/// would be a very different program. Warns (but does not refuse) when
/// the host is not obviously local, since an RTSA server normally sits
/// on the operator's own network.
///
/// The one copy of this check: `HttpSource` construction and the
/// link-budget probe both call it, so a future tightening (scheme, host
/// policy) cannot apply to one path and not the other.
pub(crate) fn validate_base_url(base_url: &str) -> Result<url::Url> {
    let parsed_url = url::Url::parse(base_url)
        .map_err(|_| Error::Protocol(format!("Invalid base URL format: {}", base_url)))?;

    match parsed_url.scheme() {
        "http" | "https" => {}
        _ => {
            return Err(Error::Protocol(format!(
                "Only HTTP/HTTPS URLs are allowed, got: {}",
                parsed_url.scheme()
            )));
        }
    }

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

    Ok(parsed_url)
}

/// Represents server information obtained from the `/info` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub title: String,
    pub uuid: String,
    pub port: u16,
    pub mission: String,
}

/// Represents user information and authentication token from the `/user` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub name: String,
    pub email: Option<String>,
    pub token: String,
    pub groups: Vec<String>,
}

/// Remote Config licensing status detection result
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum RemoteConfigStatus {
    /// License is active and remote config is available
    Active,
    /// License is not active (403 Forbidden or similar)
    NotLicensed,
    /// Authentication is required before accessing remote config
    AuthenticationRequired,
    /// Unknown status due to network or other technical errors
    Unknown(String),
}

impl RemoteConfigStatus {
    /// Returns true if remote config functionality is available
    pub fn is_available(&self) -> bool {
        matches!(self, RemoteConfigStatus::Active)
    }

    /// Returns a human-readable status description
    pub fn description(&self) -> &str {
        match self {
            RemoteConfigStatus::Active => "Remote Config license is active",
            RemoteConfigStatus::NotLicensed => "Remote Config license not available",
            RemoteConfigStatus::AuthenticationRequired => "Authentication required",
            RemoteConfigStatus::Unknown(_) => "Unknown status",
        }
    }
}

/// Configuration change verification result
#[derive(Debug, Clone)]
pub struct ConfigChangeVerification {
    /// Whether the configuration change was successfully applied
    pub applied: bool,
    /// The parameter that was tested
    pub parameter: String,
    /// Original value before change
    pub original_value: f64,
    /// Test value that was applied
    pub test_value: f64,
    /// Current value after change attempt
    pub current_value: f64,
    /// Any error encountered during verification
    pub error: Option<String>,
}

/// Represents the list of available input streams from the `/inputs` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputInfo {
    pub inputs: Vec<String>,
}

/// Defines the request body for creating a new processed input stream via the `/inputs` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub struct CreateInputRequest {
    /// The name of the original input stream to process.
    pub input: String,
    /// The type of processing to apply to the input stream.
    #[serde(rename = "type")]
    pub input_type: InputProcessingType,
}

/// Defines the available processing types for creating new input streams.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputProcessingType {
    Average,
    Maxhold,
    Minhold,
    Maxfall,
    Histogram,
    Waterfall,
}

/// Represents the payload for pushing IQ samples to the RTSA device via the `/sample` endpoint.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TxSampleRequest<'a> {
    pub start_time: f64,
    pub end_time: f64,
    pub start_frequency: f64,
    pub end_frequency: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_frequency: Option<f64>,
    pub min_power: f32,
    pub max_power: f32,
    pub sample_size: u32,
    pub sample_depth: u32,
    pub unit: String,
    pub payload: String,
    pub push: bool,
    pub samples: &'a [f32],
}

/// Defines the type of control command being sent to the `/control` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ControlType {
    #[default]
    Streaming,
    Capture,
    Antenna,
    Recording,
    Mission,
}

/// Defines the command for starting or stopping data streaming.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingControl {
    pub start: bool,
    #[serde(rename = "type")]
    pub control_type: ControlType,
}

/// Defines the command for configuring capture parameters like frequency and span.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CaptureControl {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_center: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_span: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_start: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_end: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_bins: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_level: Option<f32>,
    #[serde(rename = "type")]
    pub control_type: ControlType,
}

/// How far the read-back `centerfreq0` may sit from the requested value
/// before [`HttpEndpointsClient::apply_capture_config`] warns. The field's
/// step is 1 kHz, so one step of slack absorbs any device-side rounding
/// while still catching a write that did not take (which leaves the old
/// centre, typically MHz away).
const CENTERFREQ_CONFIRM_TOLERANCE_HZ: f64 = 1000.0;

/// How far the read-back `reflevel0` may sit from the requested value
/// before warning. The field's step is 0.5 dB; half a step catches an
/// ignored write without false-flagging device-side rounding.
const REFLEVEL_CONFIRM_TOLERANCE_DB: f64 = 0.25;

/// A retune request for [`HttpEndpointsClient::apply_capture_config`].
///
/// Each field is applied to the `main` group of the discovered receiver
/// block via `/remoteconfig` only when it is `Some`; a `None` field is
/// left untouched on the device. This is deliberately the SPECTRAN V6
/// field set (`centerfreq0` / `decimation0` / `reflevel0`), the names the
/// device actually exposes — not the `/control` capture fields
/// (`frequencyCenter` / `frequencySpan` / `referenceLevel`).
///
/// The distinction that matters: `/control` answers `success=true`
/// whether or not any block applied the command. The same device has
/// been measured both honouring a full-tuple `/control` capture and
/// ignoring one, in different mission states, with identical responses —
/// so nothing in that reply proves a retune happened. This path exists
/// because `/remoteconfig` can be read back.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CaptureConfig {
    /// `main/centerfreq0`, in Hz.
    pub center_freq_hz: Option<f64>,
    /// `main/decimation0`, the "Span" enum index (0 = Full, 9 = 1 / 512).
    /// Use [`crate::decimation_index_for_bandwidth`] to derive it from a
    /// requested span, or [`crate::decimation_index_for_rate`] from a rate.
    pub decimation_index: Option<usize>,
    /// `main/reflevel0`, in dBm.
    pub reflevel_dbm: Option<f64>,
}

/// The device values [`HttpEndpointsClient::apply_capture_config`] read
/// back from `/remoteconfig` **after** its write, so a caller can confirm
/// the write took (a wrong receiver name returns HTTP 200 and changes
/// nothing). Each field is `None` when the leaf was absent from the block.
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedCaptureConfig {
    /// The `Block_*` receiver name the write targeted.
    pub receiver_name: String,
    /// `main/centerfreq0` as the device now reports it, in Hz.
    pub center_freq_hz: Option<f64>,
    /// `main/decimation0` index as the device now reports it.
    pub decimation_index: Option<usize>,
    /// `main/reflevel0` as the device now reports it, in dBm.
    pub reflevel_dbm: Option<f64>,
}

/// Defines the command for starting or stopping antenna rotation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntennaControl {
    pub rotate: bool,
    #[serde(rename = "type")]
    pub control_type: ControlType,
}

/// Defines the command for starting or stopping a recording.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingControl {
    pub start: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(rename = "type")]
    pub control_type: ControlType,
}

/// Defines the command for saving or reloading a mission configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionControl {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reload: Option<bool>,
    #[serde(rename = "type")]
    pub control_type: ControlType,
}

/// Represents the overall health status of the device from the `/healthstatus` endpoint.
///
/// Note: The healthstatus endpoint returns a ConfigItem directly, not wrapped in a request structure
pub type HealthStatus = ConfigItem;

/// Represents the health status of an individual processing block within the device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockHealth {
    pub info: BlockInfo,
    pub status: HashMap<String, serde_json::Value>,
    pub health: HealthState,
    pub settings: Option<HashMap<String, serde_json::Value>>,
    pub components: Option<HashMap<String, BlockHealth>>,
}

/// Contains basic identifying information for a processing block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockInfo {
    pub date: f64, // Timestamp of last update
    pub name: String,
    pub category: String,
    pub title: String,
    pub description: String,
    pub uuid: String,
}

/// Represents the health state and any error messages for a block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthState {
    pub state: DeviceState,
    pub error: String,
}

/// Defines the possible operational states of a device block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum DeviceState {
    Unknown,
    Idle,
    Booting,
    Ready,
    Starting,
    Operational,
    Running,
    Warning,
    Critical,
}

impl DeviceState {
    /// Returns a string representation of the device state.
    pub fn as_str(&self) -> &'static str {
        match self {
            DeviceState::Unknown => "unknown",
            DeviceState::Idle => "idle",
            DeviceState::Booting => "booting",
            DeviceState::Ready => "ready",
            DeviceState::Starting => "starting",
            DeviceState::Operational => "operational",
            DeviceState::Running => "running",
            DeviceState::Warning => "warning",
            DeviceState::Critical => "critical",
        }
    }
}

/// Represents the entire configuration tree of the device from the `/remoteconfig` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigResponse {
    pub request: u32,
    pub config: ConfigItem,
}

/// Represents a single item within the device's configuration tree.
///
/// This is a recursive enum that can represent groups of settings, booleans, numbers, etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ConfigItem {
    Group {
        name: String,
        label: String,
        #[serde(default)]
        flags: String,
        items: Vec<ConfigItem>,
    },
    Bool {
        name: String,
        #[serde(default)]
        label: String,
        #[serde(default)]
        flags: String,
        value: bool,
        default: bool,
        #[serde(default)]
        text_off: Option<String>,
        #[serde(default)]
        text_on: Option<String>,
    },
    Number {
        name: String,
        label: String,
        #[serde(default)]
        flags: String,
        value: f64,
        default: f64,
        min: Option<f64>,
        max: Option<f64>,
        step: Option<f64>,
        unit: Option<String>,
    },
    Float {
        name: String,
        label: String,
        #[serde(default)]
        flags: String,
        value: f64,
        default: f64,
        min: Option<f64>,
        max: Option<f64>,
        step: Option<f64>,
        unit: Option<String>,
    },
    Integer {
        name: String,
        label: String,
        #[serde(default)]
        flags: String,
        value: i64,
        default: i64,
        min: Option<i64>,
        max: Option<i64>,
        step: Option<i64>,
        unit: Option<String>,
    },
    String {
        name: String,
        label: String,
        #[serde(default)]
        flags: String,
        value: String,
        default: String,
        pattern: Option<String>,
    },
    Enum {
        name: String,
        label: String,
        #[serde(default)]
        flags: String,
        value: i64,
        default: i64,
        values: String, // Comma-separated list
    },
    Button {
        name: String,
        label: String,
        #[serde(default)]
        flags: String,
    },
    FrequencyProfiles {
        name: String,
        label: String,
        #[serde(default)]
        flags: String,
        #[serde(default)]
        profiles: Option<Vec<String>>, // Make optional since structure is unknown
    },
}

/// Defines the parameters for a data stream from the `/stream` endpoint.
#[derive(Debug, Clone)]
pub struct StreamParams {
    pub format: crate::http_streaming::StreamFormat,
    pub limit: Option<u32>,
    pub rate_reduction: Option<u32>,
    pub rate_adaption: Option<bool>,
    pub input: Option<String>,
    pub scale: Option<f64>,
}

impl StreamParams {
    /// The full `/stream` URL these parameters select on `base_url`.
    ///
    /// `pub(crate)` so every `/stream` URL in the crate — the endpoints
    /// client, `HttpSource`, and the link-budget probe — is assembled by
    /// this one function and cannot drift. The query is never empty
    /// (`format` is always present), so the `?` is unconditional.
    pub(crate) fn stream_url(&self, base_url: &str) -> String {
        format!("{}/stream?{}", base_url, self.build_query_string())
    }

    /// Refuse parameter values the server would reject or misread,
    /// before any connection is opened.
    ///
    /// The builders accept any integer for `rate_reduction`, so this is
    /// where `0` is caught: it is not "no reduction" — that is `1`, or
    /// omitting the parameter — and sending it on would fail the stream
    /// open with an opaque HTTP error, or worse, be read by the server
    /// in some way this crate does not model. Every `/stream` opener
    /// (`HttpEndpointsClient::start_stream`, `HttpSource`, the
    /// link-budget probe) calls this, so the value cannot reach the wire.
    pub fn validate(&self) -> crate::Result<()> {
        Self::validate_rate_reduction(self.rate_reduction)
    }

    /// The `rate_reduction` half of [`Self::validate`], for a caller that
    /// holds the factor before it holds a `StreamParams`.
    pub(crate) fn validate_rate_reduction(factor: Option<u32>) -> crate::Result<()> {
        if factor == Some(0) {
            return Err(crate::Error::Config(
                "rate_reduction must be at least 1: 0 is not \"no reduction\" (that is 1, or \
                 leaving it unset) but a value the server would refuse"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Builds the percent-encoded query string for the stream request.
    fn build_query_string(&self) -> String {
        let mut ser = url::form_urlencoded::Serializer::new(String::new());

        ser.append_pair("format", self.format.as_str());

        if let Some(limit) = self.limit {
            ser.append_pair("limit", &limit.to_string());
        }
        if let Some(rate_reduction) = self.rate_reduction {
            ser.append_pair("rate_reduction", &rate_reduction.to_string());
        }
        if let Some(rate_adaption) = self.rate_adaption {
            ser.append_pair("rate_adaption", if rate_adaption { "1" } else { "0" });
        }
        if let Some(ref input) = self.input {
            ser.append_pair("input", input);
        }
        if let Some(scale) = self.scale {
            ser.append_pair("scale", &scale.to_string());
        }

        ser.finish()
    }
}

/// A builder for creating `StreamParams` with a fluent API.
#[derive(Debug, Clone)]
pub struct StreamParamsBuilder {
    params: StreamParams,
}

impl Default for StreamParamsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamParamsBuilder {
    /// Create a builder with the `/stream` endpoint defaults (Float32
    /// binary format, no limit/rate-reduction/scale).
    ///
    /// The default wire format is **Float32**, not JSON. An earlier
    /// revision defaulted to JSON, which is the slowest and most fragile
    /// format for high-rate IQ (a 61 MSPS stream rendered as ASCII
    /// decimal is an order of magnitude more bandwidth, and a dropped
    /// connection mid-document surfaces as confusing serde "EOF while
    /// parsing" errors) — any call site that forgot an explicit
    /// `.format(..)` silently inherited that footgun in production. JSON
    /// remains available via `.format(StreamFormat::Json)` for debugging,
    /// where human-readable payloads are the point.
    pub fn new() -> Self {
        Self {
            params: StreamParams {
                format: crate::http_streaming::StreamFormat::Float32,
                limit: None,
                rate_reduction: None,
                rate_adaption: None,
                input: None,
                scale: None,
            },
        }
    }

    /// Set the wire format (`json`, `int16`, `float16`, `float32`).
    #[must_use]
    pub fn format(mut self, format: crate::http_streaming::StreamFormat) -> Self {
        self.params.format = format;
        self
    }

    /// Cap the stream at this many packets before the server closes the
    /// connection (`?limit=N`).
    #[must_use]
    pub fn limit(mut self, limit: u32) -> Self {
        self.params.limit = Some(limit);
        self
    }

    /// Ask the server to thin the sample rate by this integer factor
    /// (`?rate_reduction=N`).
    #[must_use]
    pub fn rate_reduction(mut self, factor: u32) -> Self {
        self.params.rate_reduction = Some(factor);
        self
    }

    /// Enable or disable the server's automatic rate adaptation
    /// (`?rate_adaption=0|1`).
    #[must_use]
    pub fn rate_adaption(mut self, enabled: bool) -> Self {
        self.params.rate_adaption = Some(enabled);
        self
    }

    /// Select a named input stream other than the server's default
    /// (`?input=`).
    #[must_use]
    pub fn input(mut self, input: String) -> Self {
        self.params.input = Some(input);
        self
    }

    /// Set the server-side integer encode multiplier (`?scale=N`); see
    /// [`StreamParams`] for the encode/decode semantics.
    #[must_use]
    pub fn scale(mut self, scale: f64) -> Self {
        self.params.scale = Some(scale);
        self
    }

    /// Consume the builder and produce the final [`StreamParams`].
    pub fn build(self) -> StreamParams {
        self.params
    }
}

/// Provides a client for interacting with the Aaronia RTSA Suite HTTP API.
#[derive(Clone)]
pub struct HttpEndpointsClient {
    client: Client,
    base_url: String,
    auth: AuthMethod,
}

impl HttpEndpointsClient {
    /// Per-request timeout for control-plane operations (config, health,
    /// recording control, …). Streaming requests deliberately have **no**
    /// total timeout: `reqwest`'s request timeout covers the entire
    /// response body, so a client-level timeout would kill a continuous
    /// `/stream` connection mid-flight (this bit us at 120 s).
    const CONTROL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    /// Creates a new `HttpEndpointsClient` optimized for high-performance RTSA streaming.
    ///
    /// This constructor is optimized for both control operations and high-throughput streaming:
    /// - Control operations (device configuration, health status, recording control)
    /// - High-throughput streaming (up to 250M samples/sec capability)
    /// - Long-duration streaming sessions with appropriate timeouts
    /// - Efficient connection pooling (control + streaming channels)
    ///
    /// # Arguments
    ///
    /// * `base_url` - The base URL of the Aaronia RTSA device (e.g., "http://127.0.0.1:8080").
    /// * `auth` - The authentication method to use.
    pub fn new(base_url: String, auth: AuthMethod) -> Result<Self> {
        // Shared RTSA client settings; see `rtsa_client_builder` for why
        // there is no client-level timeout. 5 s connect: fast failure on
        // an unreachable server.
        let client = rtsa_client_builder(std::time::Duration::from_secs(5)).build()?;

        Ok(Self {
            client,
            base_url,
            auth,
        })
    }

    /// Tests the connection to the RTSA device by fetching the `/info` endpoint.
    pub async fn test_connection(&self) -> Result<()> {
        info!("Testing connection to {}...", self.base_url);
        let url = format!("{}/info", self.base_url);
        let response = self.control_request(self.client.get(&url)).send().await?;
        Self::ensure_success("Connection test", response)?;
        info!("Connection successful.");
        Ok(())
    }

    /// Applies the configured authentication method to a `reqwest::RequestBuilder`.
    fn apply_auth(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        self.auth.apply_to(builder)
    }

    /// Auth + per-request timeout for control-plane requests. Use this for
    /// everything except `/stream` (which must not have a body timeout).
    fn control_request(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        self.apply_auth(builder).timeout(Self::CONTROL_TIMEOUT)
    }

    /// Map a non-success response to an error carrying a typed
    /// [`HttpStatusError`], or pass the response through.
    ///
    /// `pub(crate)` so the link-budget probe shares the same
    /// status-to-error mapping (and any future per-status special-casing)
    /// instead of carrying its own copy.
    pub(crate) fn ensure_success(
        context: &str,
        response: reqwest::Response,
    ) -> Result<reqwest::Response> {
        let status = response.status();
        if status.is_success() {
            Ok(response)
        } else {
            Err(Error::Http {
                status,
                context: context.to_string(),
            })
        }
    }

    /// Fetches server information from the `/info` endpoint.
    pub async fn get_info(&self) -> Result<ServerInfo> {
        let url = format!("{}/info", self.base_url);
        let response = self.control_request(self.client.get(&url)).send().await?;
        let response = Self::ensure_success("Info request", response)?;

        let info: ServerInfo = response.json().await?;
        debug!("Retrieved server info: {} ({})", info.title, info.name);
        Ok(info)
    }

    /// Fetches user information and an authentication token from the `/user` endpoint.
    pub async fn get_user(&self) -> Result<UserInfo> {
        let url = format!("{}/user", self.base_url);
        let response = self.control_request(self.client.get(&url)).send().await?;
        let response = Self::ensure_success("User request", response)?;

        let user: UserInfo = response.json().await?;
        // Never log any part of the token — even a prefix narrows a
        // brute-force search and tends to end up in shipped log files.
        debug!(
            "Retrieved user info: {} (token: [REDACTED, {} chars])",
            user.name,
            user.token.len()
        );
        Ok(user)
    }

    /// Fetches the list of available input streams from the `/inputs` endpoint.
    pub async fn get_inputs(&self) -> Result<Vec<String>> {
        let url = format!("{}/inputs", self.base_url);
        let response = self.control_request(self.client.get(&url)).send().await?;
        let response = Self::ensure_success("Inputs request", response)?;

        let input_info: InputInfo = response.json().await?;
        debug!("Retrieved {} available inputs", input_info.inputs.len());
        Ok(input_info.inputs)
    }

    /// Creates a new processed input stream via the `/inputs` endpoint.
    pub async fn create_input(
        &self,
        original_input: &str,
        processing_type: InputProcessingType,
    ) -> Result<String> {
        let url = format!("{}/inputs", self.base_url);
        let request_body = CreateInputRequest {
            input: original_input.to_string(),
            input_type: processing_type,
        };

        let request = self
            .control_request(self.client.post(&url))
            .json(&request_body);
        let response = Self::ensure_success("Create input request", request.send().await?)?;

        let result: serde_json::Value = response.json().await?;
        let new_input_name = result["name"]
            .as_str()
            .ok_or_else(|| Error::Protocol("Invalid create input response".to_string()))?
            .to_string();

        info!("Created new input stream: {}", new_input_name);
        Ok(new_input_name)
    }

    /// Fetches a single sample from the `/sample` endpoint.
    pub async fn get_sample(&self, input: Option<&str>) -> Result<PacketMetadata> {
        // Percent-encode values; input names come from the server and may
        // contain characters that would corrupt a hand-built query string.
        // The Serializer is scoped so the future stays `Send`.
        let url = {
            let mut url = format!("{}/sample", self.base_url);
            if let Some(input_name) = input {
                let mut ser = url::form_urlencoded::Serializer::new(String::new());
                ser.append_pair("input", input_name);
                url.push('?');
                url.push_str(&ser.finish());
            }
            url
        };
        let request = self.control_request(self.client.get(&url));

        let response = Self::ensure_success("Sample request", request.send().await?)?;

        let sample: PacketMetadata = response.json().await?;
        debug!("Retrieved single sample: {} samples", sample.sample_size);
        Ok(sample)
    }

    /// Fetches multiple samples from the `/samples` endpoint.
    pub async fn get_samples(
        &self,
        limit: Option<u32>,
        input: Option<&str>,
    ) -> Result<Vec<PacketMetadata>> {
        // Serializer scoped so the future stays `Send` (it holds a
        // non-Send trait object internally).
        let url = {
            let mut url = format!("{}/samples", self.base_url);
            let mut ser = url::form_urlencoded::Serializer::new(String::new());
            if let Some(limit_val) = limit {
                ser.append_pair("limit", &limit_val.to_string());
            }
            if let Some(input_name) = input {
                ser.append_pair("input", input_name);
            }
            let query = ser.finish();
            if !query.is_empty() {
                url.push('?');
                url.push_str(&query);
            }
            url
        };
        let request = self.control_request(self.client.get(&url));

        let response = Self::ensure_success("Samples request", request.send().await?)?;

        let samples: Vec<PacketMetadata> = response.json().await?;
        debug!("Retrieved {} samples", samples.len());
        Ok(samples)
    }

    /// Pushes multiple IQ samples to the `/sample` endpoint for transmitting to the RTSA device.
    pub async fn push_samples(&self, request_payload: &TxSampleRequest<'_>) -> Result<()> {
        let url = format!("{}/sample", self.base_url);
        let request = self
            .control_request(self.client.post(&url))
            .json(request_payload);
        Self::ensure_success("Push samples request", request.send().await?)?;

        let num_complex = request_payload.samples.len() / 2;
        debug!("Pushed {} complex samples to device", num_complex);
        Ok(())
    }

    /// Sends a command to start or stop data streaming via the `/control` endpoint.
    pub async fn control_streaming(&self, start: bool) -> Result<()> {
        let url = format!("{}/control", self.base_url);
        let command = StreamingControl {
            start,
            control_type: ControlType::Streaming,
        };

        let request = self.control_request(self.client.put(&url)).json(&command);
        Self::ensure_success("Streaming control", request.send().await?)?;

        info!("Streaming {}", if start { "started" } else { "stopped" });
        Ok(())
    }

    /// Gracefully shut down the RTSA-Suite application via the `/app/process`
    /// endpoint with `{"running": false}`.
    ///
    /// **This targets the RTSA *application's* HTTP control surface, not the
    /// HTTP server *block* embedded in a mission graph.** Both expose REST
    /// endpoints — see the official forum thread "Close RTSA application
    /// with an api". Sending this PUT to a mission's HTTP server block port
    /// has no effect; you have to point it at the RTSA application port.
    ///
    /// On success the RTSA process exits cleanly. On failure the process
    /// continues running and the underlying error is propagated.
    pub async fn shutdown_application(&self) -> Result<()> {
        #[derive(serde::Serialize)]
        struct AppProcess<'a> {
            running: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            reason: Option<&'a str>,
        }
        let url = format!("{}/app/process", self.base_url);
        let body = AppProcess {
            running: false,
            reason: None,
        };
        let request = self.control_request(self.client.put(&url)).json(&body);
        Self::ensure_success(
            "Application shutdown via /app/process (confirm the URL points at the \
             RTSA application's HTTP control endpoint, not at a mission's HTTP \
             server block)",
            request.send().await?,
        )?;
        info!("RTSA application shutdown requested");
        Ok(())
    }

    /// Push a "simpleconfig" PUT to `/remoteconfig`.
    ///
    /// Both the SDR++ `spectran_http_source` and SDRangel
    /// `aaroniartsainput` plugins use this shorter shape:
    /// ```json
    /// {
    ///   "receiverName": "Block_IQDemodulator_0",
    ///   "simpleconfig": { "main": { "centerfreq": 100e6,
    ///                               "samplerate": 10e6,
    ///                               "spanfreq": 10e6 } }
    /// }
    /// ```
    /// instead of the full `{ "request": 0, "config": { "type": "group",
    /// "items": [...] } }` form. `receiverName` must match a block in the
    /// running mission — use [`Self::find_iq_demodulator_block_name`] to
    /// auto-discover it.
    ///
    /// `main` is the typical group, but any subset of the block's
    /// configuration tree may be passed via `config_groups`. Keys are
    /// group names ("main", "device", "calibration"); values are field
    /// → `serde_json::Value` maps. Several groups in one call are
    /// applied together.
    ///
    /// Enum fields take either the label the device reports or its
    /// index in that item's `values` list.
    ///
    /// **A wrong `receiver_name` is silently accepted.** Naming a block
    /// that is not in the running mission returns HTTP 200 and changes
    /// nothing, so this returns `Ok(())` for a write that did not
    /// happen. The server offers nothing to check against, so resolve
    /// the name once from [`Self::get_config`] (or
    /// [`Self::find_iq_demodulator_block_name`]) when the session
    /// starts, rather than hard-coding a block name and trusting the
    /// status code. Read the value back afterwards when a write has to
    /// be certain.
    pub async fn simple_remote_config(
        &self,
        receiver_name: &str,
        config_groups: serde_json::Map<String, serde_json::Value>,
    ) -> Result<()> {
        let body = serde_json::json!({
            "receiverName": receiver_name,
            "simpleconfig": serde_json::Value::Object(config_groups),
        });
        let url = format!("{}/remoteconfig", self.base_url);
        let response = self
            .control_request(self.client.put(&url))
            .json(&body)
            .send()
            .await?;
        Self::ensure_success("simpleconfig PUT to /remoteconfig", response)?;
        Ok(())
    }

    /// Discover the running mission's IQ-demodulator block name (e.g.
    /// `Block_IQDemodulator_0`) by querying `/remoteconfig` and walking
    /// the config tree depth-first for an item whose `name` starts with
    /// `Block_IQDemodulator`.
    ///
    /// Both the SDR++ and SDRangel plugins do this auto-discovery before
    /// they can write any config — the block name is mission-specific and
    /// not predictable by the client.
    ///
    /// The earlier implementation (A16) only scanned the top-level
    /// `config.items` array and silently returned "not found" when the
    /// block was nested inside a sub-group (which happens on every
    /// real RTSA mission that uses scene grouping). The walk now
    /// recurses through every nested `items` array.
    pub async fn find_iq_demodulator_block_name(&self) -> Result<String> {
        let url = format!("{}/remoteconfig", self.base_url);
        let bytes = self
            .control_request(self.client.get(&url))
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        let document: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| Error::Protocol(e.to_string()))?;
        let root_items = document
            .get("config")
            .and_then(|c| c.get("items"))
            .and_then(|i| i.as_array())
            .ok_or_else(|| {
                Error::Protocol("/remoteconfig response had no `config.items` array".to_string())
            })?;
        find_config_item_by_name_prefix(root_items, "Block_IQDemodulator").ok_or_else(|| {
            Error::Protocol(
                "no Block_IQDemodulator_* block found anywhere in current mission's config tree"
                    .to_string(),
            )
        })
    }

    /// `GET /remoteconfig` as a raw `serde_json::Value`.
    ///
    /// The typed [`Self::get_config`] deserialises into [`ConfigResponse`];
    /// this keeps the untyped tree so the block-discovery and read-back
    /// walkers can look up arbitrary leaves by name without every device's
    /// full schema being modelled.
    async fn get_remoteconfig_document(&self) -> Result<serde_json::Value> {
        let url = format!("{}/remoteconfig", self.base_url);
        let bytes = self
            .control_request(self.client.get(&url))
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        serde_json::from_slice(&bytes).map_err(|e| Error::Protocol(e.to_string()))
    }

    /// Discover the mission block whose config carries the leaf `field`
    /// (e.g. `centerfreq0`) and return its `Block_*` name, suitable as the
    /// `receiverName` for [`Self::simple_remote_config`] /
    /// [`Self::apply_capture_config`].
    ///
    /// Prefer this over [`Self::find_iq_demodulator_block_name`] whenever
    /// the write target is known: that helper only matches the
    /// `Block_IQDemodulator` prefix and returns an error on devices whose
    /// tuner is a different block category (a SPECTRAN V6 ECO's tuner is
    /// `Block_Spectran_V6Eco_0`, a `spectrumanalyzer` block, which the
    /// prefix scan never finds). Matching on the field the write will
    /// target locates the right block regardless of its category.
    pub async fn find_block_name_with_field(&self, field: &str) -> Result<String> {
        let document = self.get_remoteconfig_document().await?;
        let root_items = document
            .get("config")
            .and_then(|c| c.get("items"))
            .and_then(|i| i.as_array())
            .ok_or_else(|| {
                Error::Protocol("/remoteconfig response had no `config.items` array".to_string())
            })?;
        find_block_name_carrying_field(root_items, field, None).ok_or_else(|| {
            Error::Protocol(format!(
                "no Block_* group carrying `{field}` found in the current mission's config tree"
            ))
        })
    }

    /// Retune the device — centre frequency, span and/or reference level —
    /// via `/remoteconfig`, then read the values back and confirm the write
    /// took.
    ///
    /// This is the reliable retune path for a SPECTRAN V6 under RTSA-Suite.
    /// It targets the `main` group's real field names (`centerfreq0`,
    /// `decimation0`, `reflevel0`), auto-discovers the receiver block via
    /// [`Self::find_block_name_with_field`], and applies only the `Some`
    /// fields of `request` (a `None` leaves that device value alone).
    ///
    /// **Why the read-back is not optional.** A `/remoteconfig` PUT naming
    /// a block that is not in the running mission returns HTTP 200 and
    /// changes nothing (see [`Self::simple_remote_config`]), so the status
    /// code proves nothing. After writing, this re-reads the block and
    /// compares each requested field against what the device now reports:
    /// a match logs at info, a mismatch logs a warning naming the
    /// requested and actual values. The returned [`AppliedCaptureConfig`]
    /// carries the read-back so the caller can sync its own state (e.g. the
    /// processing sample rate) to what the device actually did.
    ///
    /// Returns an error only if discovery, the write, or the read-back HTTP
    /// call itself fails — a write that is silently ignored is reported
    /// through the warning log and the returned values, not as an `Err`,
    /// since the device still answered.
    pub async fn apply_capture_config(
        &self,
        request: &CaptureConfig,
    ) -> Result<AppliedCaptureConfig> {
        let block = self.find_block_name_with_field("centerfreq0").await?;

        let mut main = serde_json::Map::new();
        if let Some(hz) = request.center_freq_hz {
            main.insert("centerfreq0".to_string(), serde_json::json!(hz));
        }
        if let Some(idx) = request.decimation_index {
            // Enum fields take the index directly (see `simple_remote_config`).
            main.insert("decimation0".to_string(), serde_json::json!(idx));
        }
        if let Some(dbm) = request.reflevel_dbm {
            main.insert("reflevel0".to_string(), serde_json::json!(dbm));
        }

        if main.is_empty() {
            // Nothing requested: read the block back so the caller still
            // learns the current state, but do not PUT an empty group.
            return self.read_capture_config(&block).await;
        }

        let mut groups = serde_json::Map::new();
        groups.insert("main".to_string(), serde_json::Value::Object(main));
        self.simple_remote_config(&block, groups).await?;

        let applied = self.read_capture_config(&block).await?;

        // Confirm each requested field against the read-back. Center is a
        // float snapped to the device's frequency step; decimation and
        // reflevel are exact (an enum index and a 0.5 dB grid).
        if let Some(want) = request.center_freq_hz {
            match applied.center_freq_hz {
                Some(got) if (got - want).abs() <= CENTERFREQ_CONFIRM_TOLERANCE_HZ => {
                    info!("RTSA {block}: centerfreq0 = {got:.0} Hz (requested {want:.0})");
                }
                Some(got) => warn!(
                    "RTSA {block}: centerfreq0 did not take — requested {want:.0} Hz, device \
                     reports {got:.0} Hz (wrong receiverName, or the field is locked?)"
                ),
                None => warn!("RTSA {block}: centerfreq0 missing from read-back"),
            }
        }
        if let Some(want) = request.decimation_index {
            match applied.decimation_index {
                Some(got) if got == want => {
                    info!("RTSA {block}: decimation0 = {got} (requested {want})");
                }
                Some(got) => warn!(
                    "RTSA {block}: decimation0 did not take — requested index {want}, device \
                     reports {got}"
                ),
                None => warn!("RTSA {block}: decimation0 missing from read-back"),
            }
        }
        if let Some(want) = request.reflevel_dbm {
            match applied.reflevel_dbm {
                Some(got) if (got - want).abs() <= REFLEVEL_CONFIRM_TOLERANCE_DB => {
                    info!("RTSA {block}: reflevel0 = {got} dBm (requested {want})");
                }
                Some(got) => warn!(
                    "RTSA {block}: reflevel0 did not take — requested {want} dBm, device reports \
                     {got} dBm"
                ),
                None => warn!("RTSA {block}: reflevel0 missing from read-back"),
            }
        }

        Ok(applied)
    }

    /// Read `centerfreq0` / `decimation0` / `reflevel0` back from a block's
    /// subtree in the current `/remoteconfig` tree.
    async fn read_capture_config(&self, block: &str) -> Result<AppliedCaptureConfig> {
        let document = self.get_remoteconfig_document().await?;
        let root_items = document
            .get("config")
            .and_then(|c| c.get("items"))
            .and_then(|i| i.as_array())
            .ok_or_else(|| {
                Error::Protocol("/remoteconfig response had no `config.items` array".to_string())
            })?;
        let block_items = find_group_items(root_items, block).ok_or_else(|| {
            Error::Protocol(format!("block `{block}` vanished from the config tree"))
        })?;
        Ok(AppliedCaptureConfig {
            receiver_name: block.to_string(),
            center_freq_hz: read_config_leaf_value(block_items, "centerfreq0"),
            decimation_index: read_config_leaf_value(block_items, "decimation0")
                .map(|v| v as usize),
            reflevel_dbm: read_config_leaf_value(block_items, "reflevel0"),
        })
    }

    /// Sends a command to start or stop antenna rotation via the `/control` endpoint.
    pub async fn control_antenna(&self, rotate: bool) -> Result<()> {
        let url = format!("{}/control", self.base_url);
        let command = AntennaControl {
            rotate,
            control_type: ControlType::Antenna,
        };

        let request = self.control_request(self.client.put(&url)).json(&command);
        Self::ensure_success("Antenna control", request.send().await?)?;

        info!(
            "Antenna rotation {}",
            if rotate { "started" } else { "stopped" }
        );
        Ok(())
    }

    /// Configures capture parameters via the `/control` endpoint.
    ///
    /// **Frequency changes need `frequencyCenter` and `frequencySpan`
    /// together.** RTSA servers return `{"success":true}` for a capture
    /// `PUT` that carries only one of the two frequency fields but
    /// silently ignore it; the retune applies only when both are present
    /// (verified live against RTSA-Suite PRO driving a SPECTRAN V6 ECO —
    /// center-only and span-only PUTs each no-opped, center+span
    /// applied). `referenceLevel` does apply on its own. A lone frequency
    /// field logs a warning here and is still sent, since other server
    /// versions may accept it.
    pub async fn configure_capture(&self, config: CaptureControl) -> Result<()> {
        let has_range = config.frequency_start.is_some() && config.frequency_end.is_some();
        if !has_range && (config.frequency_center.is_some() != config.frequency_span.is_some()) {
            warn!(
                "Partial capture PUT ({:?}): RTSA servers are known to accept a lone \
                 frequencyCenter/frequencySpan with success=true but silently ignore it; \
                 include both fields for the retune to apply",
                config
            );
        }

        let url = format!("{}/control", self.base_url);
        let request = self.control_request(self.client.put(&url)).json(&config);
        Self::ensure_success("Capture configuration", request.send().await?)?;

        info!("Capture configuration updated");
        Ok(())
    }

    /// Sends a command to start or stop a recording via the `/control` endpoint.
    pub async fn control_recording(&self, start: bool, filename: Option<String>) -> Result<()> {
        let url = format!("{}/control", self.base_url);
        let command = RecordingControl {
            start,
            filename,
            control_type: ControlType::Recording,
        };

        let request = self.control_request(self.client.put(&url)).json(&command);
        Self::ensure_success("Recording control", request.send().await?)?;

        info!("Recording {}", if start { "started" } else { "stopped" });
        Ok(())
    }

    /// Fetches the device's health status from the `/healthstatus` endpoint.
    pub async fn get_health_status(&self) -> Result<HealthStatus> {
        let url = format!("{}/healthstatus", self.base_url);
        let request = self.control_request(self.client.get(&url));
        let response = Self::ensure_success("Health status request", request.send().await?)?;

        let health: HealthStatus = response.json().await?;
        let item_count = match &health {
            ConfigItem::Group { items, .. } => items.len(),
            _ => 1,
        };
        debug!("Retrieved health status with {} items", item_count);
        Ok(health)
    }

    /// Fetches the complete configuration tree from the `/remoteconfig` endpoint.
    ///
    /// Reads are license-free: this is the documented behaviour and it
    /// matches live systems, which is why a successful read proves
    /// nothing about write capability.
    /// See: <https://aaronia.com/en/software-licence-remote-config>
    /// Documentation: <https://rtsa-manual.aaronia.com/en/Content/C_Operation/DDCommandCenter/RemoteConfig.htm>
    pub async fn get_config(&self) -> Result<ConfigResponse> {
        let url = format!("{}/remoteconfig", self.base_url);
        let request = self.control_request(self.client.get(&url));
        let response = Self::ensure_success("Config request", request.send().await?)?;

        let config: ConfigResponse = response.json().await?;
        debug!("Retrieved configuration tree");
        Ok(config)
    }

    /// Updates the device's configuration via the `/remoteconfig` endpoint.
    ///
    /// **LICENSING NOTE**: Aaronia sells a "Remote Config" license, and
    /// this endpoint was long documented as requiring it. Live testing
    /// contradicts that: a system whose license list contains no Remote
    /// Config entry accepted `/remoteconfig` writes and applied them
    /// (SPECTRAN V6 ECO under RTSA-Suite PRO). What the license gates is
    /// unconfirmed, so treat a failure here as possible but not
    /// certain, and handle the 401/403 case rather than assuming either
    /// outcome.
    ///
    /// **Alternatives without that license**: retuning and capture control
    /// go through `/control` ([`Self::configure_capture`]), which needs no
    /// license, and the native SDK configures the device directly. Note
    /// that a `/remoteconfig` write applies a frequency change from the
    /// frequency field alone, whereas `/control` ignores a capture request
    /// unless the center frequency and span are both present.
    ///
    /// See: <https://aaronia.com/en/software-licence-remote-config>
    /// Documentation: <https://rtsa-manual.aaronia.com/en/Content/C_Operation/DDCommandCenter/RemoteConfig.htm>
    pub async fn update_config(
        &self,
        request_id: u32,
        block_name: &str,
        items: Vec<ConfigItem>,
    ) -> Result<ConfigResponse> {
        let url = format!("{}/remoteconfig", self.base_url);

        let update_request = serde_json::json!({
            "request": request_id,
            "receivername": block_name,
            "config": {
                "type": "group",
                "name": block_name,
                "items": items
            }
        });

        let request = self
            .control_request(self.client.put(&url))
            .json(&update_request);
        let response = Self::ensure_success("Config update", request.send().await?)?;

        let config: ConfigResponse = response.json().await?;
        info!("Updated configuration for block: {}", block_name);
        Ok(config)
    }

    /// **Read-only** Remote Config licensing check.
    ///
    /// Reads `/remoteconfig` and classifies the response. Because read
    /// access works *without* the license, a successful read cannot
    /// distinguish "licensed" from "unlicensed" — in that case this
    /// returns [`RemoteConfigStatus::Unknown`] with an explanatory note.
    /// To positively confirm write capability, call
    /// [`Self::probe_remote_config_write_license`], which is explicitly
    /// documented as perturbing device state.
    ///
    /// This method never modifies the device.
    pub async fn detect_remote_config_license(&self) -> RemoteConfigStatus {
        debug!("Checking Remote Config licensing status (read-only)");

        match self.get_config().await {
            Ok(_) => RemoteConfigStatus::Unknown(
                "read access confirmed, but reads work without a license; call \
                 probe_remote_config_write_license() to verify write capability"
                    .to_string(),
            ),
            Err(e) => match e {
                Error::Http {
                    status: reqwest::StatusCode::UNAUTHORIZED,
                    ..
                } => {
                    info!("Remote Config license detection: AUTHENTICATION REQUIRED");
                    RemoteConfigStatus::AuthenticationRequired
                }
                Error::Http {
                    status: reqwest::StatusCode::FORBIDDEN,
                    ..
                } => {
                    info!("Remote Config license detection: NOT LICENSED (403 on read)");
                    RemoteConfigStatus::NotLicensed
                }
                _ => {
                    warn!("Remote Config license detection failed (read test): {}", e);
                    RemoteConfigStatus::Unknown(e.to_string())
                }
            },
        }
    }

    /// Detect Remote Config licensing by **actively testing a write**.
    ///
    /// **This mutates device state**: it temporarily changes the
    /// `reflevel` parameter by +1 dB and restores it best-effort. If the
    /// restore fails (network drop mid-probe), the device is left with the
    /// adjusted reference level. Only call this when you genuinely need
    /// proof of `/remoteconfig` write capability. Note that a system
    /// without a Remote Config license has been observed accepting those
    /// writes, so a `NotLicensed` result means "this write did not take
    /// effect", not "the license is missing". Note that retuning does
    /// **not** — [`Self::configure_capture`] goes through the license-free
    /// `/control` endpoint. (An earlier revision justified this probe by
    /// claiming unlicensed `configure_capture` calls silently no-op; live
    /// testing traced that behavior to partial capture payloads, not
    /// licensing.)
    ///
    /// # Returns
    /// - `RemoteConfigStatus::Active` - License is active, write operations work
    /// - `RemoteConfigStatus::NotLicensed` - License not active, cannot write
    /// - `RemoteConfigStatus::AuthenticationRequired` - Need authentication (401 Unauthorized)
    /// - `RemoteConfigStatus::Unknown` - Technical error or network issue
    pub async fn probe_remote_config_write_license(&self) -> RemoteConfigStatus {
        debug!("Probing Remote Config licensing status by testing write operations");

        // First check if we can read config (this works without license).
        if let Err(e) = self.get_config().await {
            return match e {
                Error::Http {
                    status: reqwest::StatusCode::UNAUTHORIZED,
                    ..
                } => {
                    info!("Remote Config license detection: AUTHENTICATION REQUIRED");
                    RemoteConfigStatus::AuthenticationRequired
                }
                _ => {
                    warn!("Remote Config license detection failed (read test): {}", e);
                    RemoteConfigStatus::Unknown(e.to_string())
                }
            };
        }

        // Now test write operations by attempting a safe configuration change.
        let test_result = self.verify_config_changes("reflevel", 1.0).await;

        match test_result {
            Ok(verification) => {
                if verification.applied {
                    info!("Remote Config license detected: ACTIVE (write operations work)");
                    RemoteConfigStatus::Active
                } else {
                    info!(
                        "Remote Config license detected: NOT LICENSED (write operations blocked)"
                    );
                    RemoteConfigStatus::NotLicensed
                }
            }
            Err(e) => match e {
                Error::Http {
                    status: reqwest::StatusCode::FORBIDDEN,
                    ..
                } => {
                    info!("Remote Config license detected: NOT LICENSED (403 Forbidden on write)");
                    RemoteConfigStatus::NotLicensed
                }
                Error::Http {
                    status: reqwest::StatusCode::UNAUTHORIZED,
                    ..
                } => {
                    info!("Remote Config license detected: AUTHENTICATION REQUIRED");
                    RemoteConfigStatus::AuthenticationRequired
                }
                _ => {
                    warn!("Remote Config license detection failed (write test): {}", e);
                    RemoteConfigStatus::NotLicensed // Assume no license if write fails
                }
            },
        }
    }

    /// Verifies that configuration changes actually take effect by performing a read-modify-write cycle.
    ///
    /// This method tests whether the Remote Config license not only allows reading configuration
    /// but also permits actual device parameter changes. It uses a safe test parameter and
    /// automatically restores the original value.
    ///
    /// # Arguments
    /// * `test_parameter` - Configuration parameter to test (e.g., "main/reflevel")
    /// * `safe_delta` - Small change to apply for testing (e.g., 1.0 for 1dB change)
    ///
    /// # Returns
    /// `ConfigChangeVerification` with detailed results of the verification process
    pub async fn verify_config_changes(
        &self,
        test_parameter: &str,
        safe_delta: f64,
    ) -> Result<ConfigChangeVerification> {
        debug!(
            "Verifying configuration changes for parameter: {}",
            test_parameter
        );

        // Step 1: Read current configuration
        let original_config = self.get_config().await?;
        let original_value =
            self.extract_parameter_value(&original_config.config, test_parameter)?;

        // Step 2: Calculate safe test value
        let test_value = original_value + safe_delta;

        // Step 3: Apply test change
        let change_result = self
            .apply_parameter_change(test_parameter, test_value)
            .await;

        // Step 4: Read back to verify.
        //
        // Deliberately *not* `?`. Once step 3 has written to the device we
        // own a side effect on the user's hardware, and the restore in
        // step 5 is the only thing that undoes it. Propagating a read-back
        // failure here would skip the restore and leave the reference
        // level permanently offset by `safe_delta` — this function's own
        // doc promises it "automatically restores the original value", and
        // a transient network blip between the write and the read-back is
        // exactly when that promise matters most. Capture the outcome,
        // always restore, then surface the error.
        let readback = async {
            let verification_config = self.get_config().await?;
            self.extract_parameter_value(&verification_config.config, test_parameter)
        }
        .await;

        // Step 5: Restore original value (best effort) — unconditionally,
        // on every path out of step 4.
        let restore_result = self
            .apply_parameter_change(test_parameter, original_value)
            .await;

        let current_value = match readback {
            Ok(v) => v,
            Err(e) => {
                if let Err(restore_err) = restore_result {
                    warn!(
                        "verify_config_changes: read-back failed ({e}) AND the restore of \
                         {test_parameter} to {original_value} also failed ({restore_err}) — \
                         the device may still be offset by {safe_delta}"
                    );
                }
                return Err(e);
            }
        };

        let applied = (current_value - test_value).abs() < 0.001; // Allow small floating point errors

        let error = if let Err(e) = change_result {
            Some(format!("Change failed: {}", e))
        } else if let Err(e) = restore_result {
            Some(format!("Restore failed: {}", e))
        } else {
            None
        };

        let verification = ConfigChangeVerification {
            applied,
            parameter: test_parameter.to_string(),
            original_value,
            test_value,
            current_value,
            error,
        };

        if applied {
            info!(
                "Configuration change verification: SUCCESS for {}",
                test_parameter
            );
        } else {
            warn!(
                "Configuration change verification: FAILED for {} (expected: {}, got: {})",
                test_parameter, test_value, current_value
            );
        }

        Ok(verification)
    }

    /// Extract a numeric parameter value from the configuration tree.
    ///
    /// `parameter_path` may be a bare leaf name (`"reflevel"`) or a
    /// slash-separated path (`"main/reflevel"`); matching is against the
    /// final segment, exactly (the previous implementation ignored the
    /// path entirely and substring-matched any item containing `"level"`,
    /// and only scanned the top level of the tree).
    fn extract_parameter_value(&self, config: &ConfigItem, parameter_path: &str) -> Result<f64> {
        let target = parameter_path
            .rsplit('/')
            .next()
            .unwrap_or(parameter_path)
            .trim();

        fn walk(item: &ConfigItem, target: &str) -> Option<f64> {
            match item {
                ConfigItem::Group { items, .. } => items.iter().find_map(|i| walk(i, target)),
                ConfigItem::Float { name, value, .. } | ConfigItem::Number { name, value, .. }
                    if name == target =>
                {
                    Some(*value)
                }
                _ => None,
            }
        }

        walk(config, target).ok_or_else(|| {
            Error::Protocol(format!(
                "Parameter {} not found in configuration tree",
                parameter_path
            ))
        })
    }

    /// Helper method to apply a parameter change
    async fn apply_parameter_change(&self, parameter: &str, value: f64) -> Result<()> {
        // Create a config item for the parameter
        let config_item = if parameter.contains("reflevel") {
            ConfigItem::Float {
                name: "reflevel".to_string(),
                label: "Reference Level".to_string(),
                flags: String::new(),
                value,
                default: -30.0,
                min: Some(-120.0),
                max: Some(30.0),
                step: Some(1.0),
                unit: Some("dBm".to_string()),
            }
        } else {
            return Err(Error::Protocol(format!(
                "Unsupported parameter type: {}",
                parameter
            )));
        };

        let _response = self.update_config(1, "main", vec![config_item]).await?;
        Ok(())
    }

    /// Starts a real-time data stream from the `/stream` endpoint.
    ///
    /// Returns a `Stream` of `StreamPacket`s.
    pub async fn start_stream(
        &self,
        params: StreamParams,
    ) -> Result<
        Box<dyn futures::Stream<Item = Result<crate::http_streaming::StreamPacket>> + Unpin + Send>,
    > {
        use crate::http_streaming::StreamParser;
        use futures::stream::StreamExt;

        params.validate()?;
        let url = params.stream_url(&self.base_url);

        info!(
            "Starting stream from {} with format: {:?}",
            url, params.format
        );

        // Deliberately `apply_auth`, not `control_request`: a per-request
        // timeout would cover the whole streamed body and kill the
        // connection mid-stream.
        let request = self.apply_auth(self.client.get(&url));
        let response =
            Self::ensure_success(&format!("Stream request to {url}"), request.send().await?)?;

        // Get the byte stream
        let byte_stream = response.bytes_stream();

        // Create parser for the specified format
        let mut parser = StreamParser::new(params.format, params.scale)?;

        // Convert the byte stream to a packet stream. One HTTP chunk can
        // complete zero, one, or several packets — `process_data` buffers
        // partials across chunks and returns everything completed, and the
        // flat_map re-flattens so no packet is dropped (the previous
        // one-packet-per-chunk mapping silently discarded the rest).
        let packet_stream = byte_stream
            .map(move |chunk_result| match chunk_result {
                Ok(chunk) => parser.process_data(&chunk),
                Err(e) => Err(Error::Protocol(format!("Stream chunk error: {}", e))),
            })
            .flat_map(|result| {
                let items: Vec<Result<crate::http_streaming::StreamPacket>> = match result {
                    Ok(packets) => packets.into_iter().map(Ok).collect(),
                    Err(e) => vec![Err(e)],
                };
                futures::stream::iter(items)
            });

        Ok(Box::new(packet_stream.boxed()))
    }

    /// Creates a new `StreamParamsBuilder` to construct streaming parameters.
    pub fn stream_params() -> StreamParamsBuilder {
        StreamParamsBuilder::new()
    }
}

/// Depth-first search of an RTSA `config.items` array (or any of its
/// nested `items` arrays) for an item whose `name` field starts with
/// `prefix`. Returns the first match's full name, or `None` if no
/// match exists anywhere in the subtree.
///
/// The RTSA `/remoteconfig` response is a tree of `ConfigItem`s
/// where any item may itself contain a nested `items` array. The
/// older top-level-only scan (A16) would silently miss blocks
/// nested inside scene groups — a common layout on real missions.
/// This walker visits every node in pre-order so the first match
/// (closest to the root) is preferred when multiple matches exist.
fn find_config_item_by_name_prefix(items: &[serde_json::Value], prefix: &str) -> Option<String> {
    for item in items {
        if let Some(name) = item.get("name").and_then(|n| n.as_str())
            && name.starts_with(prefix)
        {
            return Some(name.to_string());
        }
        // Recurse into nested item lists. Real RTSA missions nest
        // hardware blocks inside scene-group items; older clients that
        // only scanned the root array missed them.
        if let Some(nested) = item.get("items").and_then(|v| v.as_array())
            && let Some(found) = find_config_item_by_name_prefix(nested, prefix)
        {
            return Some(found);
        }
    }
    None
}

/// Depth-first search for the `Block_*` group whose config subtree
/// contains a leaf named `field`, returning that block's name.
///
/// The tunable fields live at `<Block_*>/config/main/<field>`, so the
/// `receiverName` a `/remoteconfig` write needs is the enclosing `Block_*`
/// group — not the `main` group and not the field's own name. `field`
/// found outside any `Block_*` group is ignored (it cannot be a receiver
/// name). Pre-order, so the outermost enclosing block wins when the tree
/// nests them.
///
/// This generalises [`find_config_item_by_name_prefix`]: matching on a
/// field the write will actually target finds the right block whatever its
/// category, whereas prefix-matching `Block_IQDemodulator` misses devices
/// whose tuner is, say, a `spectrumanalyzer` block (`Block_Spectran_*`).
fn find_block_name_carrying_field(
    items: &[serde_json::Value],
    field: &str,
    enclosing_block: Option<&str>,
) -> Option<String> {
    for item in items {
        let name = item.get("name").and_then(|n| n.as_str());
        if name == Some(field)
            && let Some(block) = enclosing_block
        {
            return Some(block.to_string());
        }
        // Descend, tracking the nearest enclosing Block_* ancestor.
        let next_block = match name {
            Some(n) if n.starts_with("Block_") => Some(n),
            _ => enclosing_block,
        };
        if let Some(nested) = item.get("items").and_then(|v| v.as_array())
            && let Some(found) = find_block_name_carrying_field(nested, field, next_block)
        {
            return Some(found);
        }
    }
    None
}

/// Depth-first search for the `items` array of the group named
/// `group_name`, or the group carrying that name. Returns the group's
/// child `items`, so a caller can read leaves within one block's subtree.
fn find_group_items<'a>(
    items: &'a [serde_json::Value],
    group_name: &str,
) -> Option<&'a Vec<serde_json::Value>> {
    for item in items {
        if item.get("name").and_then(|n| n.as_str()) == Some(group_name) {
            return item.get("items").and_then(|v| v.as_array());
        }
        if let Some(nested) = item.get("items").and_then(|v| v.as_array())
            && let Some(found) = find_group_items(nested, group_name)
        {
            return Some(found);
        }
    }
    None
}

/// Depth-first read of the numeric `value` of the leaf named `field`
/// within a config subtree. Works for `float`/`number`/`integer`/`enum`
/// items alike, since `serde_json`'s `as_f64` accepts JSON integers — an
/// `enum`'s value is its index. Returns `None` if the leaf is absent.
fn read_config_leaf_value(items: &[serde_json::Value], field: &str) -> Option<f64> {
    for item in items {
        if item.get("name").and_then(|n| n.as_str()) == Some(field) {
            return item.get("value").and_then(|v| v.as_f64());
        }
        if let Some(nested) = item.get("items").and_then(|v| v.as_array())
            && let Some(found) = read_config_leaf_value(nested, field)
        {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Truncate `text` to at most `max_chars` **characters** for a
    /// diagnostic preview.
    ///
    /// `&text[..max]` slices by *byte* index and panics outright when that
    /// index lands inside a multi-byte UTF-8 character — and these previews
    /// print raw RTSA responses, which carry device/mission/antenna names
    /// straight from user configuration. A single non-ASCII character in a
    /// mission title was enough to turn a diagnostic run into a panic.
    fn preview(text: &str, max_chars: usize) -> String {
        // `char_indices().nth(max_chars)` stops after `max_chars` steps and
        // hands back a byte offset that is a valid boundary by
        // construction. Counting characters instead (`chars().count()`)
        // would traverse the *whole* body just to decide whether to append
        // the marker — and these bodies are raw HTTP responses that can run
        // to megabytes.
        match text.char_indices().nth(max_chars) {
            Some((byte_idx, _)) => format!("{}...", &text[..byte_idx]),
            None => text.to_string(),
        }
    }

    /// `&text[..n]` panics when byte `n` lands inside a multi-byte UTF-8
    /// character. These previews print raw RTSA responses, which carry
    /// mission/antenna/device names straight from user configuration — a
    /// single non-ASCII character was enough to panic the diagnostic.
    #[test]
    fn preview_truncates_on_char_boundaries_not_bytes() {
        // 'é' is 2 bytes, so a 500-char string is 1000 bytes and byte 500
        // lands mid-character — the exact case that panicked.
        let multibyte = "é".repeat(500);
        assert_eq!(multibyte.len(), 1000, "precondition: bytes != chars");
        let out = preview(&multibyte, 500);
        assert_eq!(out.chars().count(), 500, "no truncation marker expected");

        let longer = "é".repeat(600);
        let out = preview(&longer, 500);
        assert!(out.ends_with("..."), "truncated preview must be marked");
        assert_eq!(out.chars().count(), 503, "500 chars + the marker");

        // A 4-byte character (emoji) straddling the limit must also be safe.
        let emoji = "🛸".repeat(600);
        let out = preview(&emoji, 500);
        assert!(out.starts_with('🛸'));
        assert!(out.ends_with("..."));

        // Short input is returned whole, unmarked.
        assert_eq!(preview("short", 500), "short");
    }

    /// Regression (field report from fpv-viewer): the params-builder
    /// default must be a binary format. A JSON default meant any call
    /// site that forgot `.format(..)` streamed high-rate IQ as ASCII —
    /// slow, and the connection died mid-document with cryptic serde
    /// EOF errors.
    #[test]
    fn stream_params_default_format_is_float32() {
        let params = StreamParamsBuilder::new().build();
        assert_eq!(
            params.format,
            crate::http_streaming::StreamFormat::Float32,
            "default stream format must be binary Float32, never JSON"
        );
    }

    #[test]
    fn test_auth_method_creation() {
        let basic = AuthMethod::Basic {
            username: "user".to_string(),
            password: "pass".to_string(),
        };
        matches!(basic, AuthMethod::Basic { .. });

        let token = AuthMethod::Token {
            token: "abc123".to_string(),
        };
        matches!(token, AuthMethod::Token { .. });
    }

    #[test]
    fn test_control_command_serialization() {
        let streaming_cmd = StreamingControl {
            start: true,
            control_type: ControlType::Streaming,
        };

        let json = serde_json::to_string(&streaming_cmd).unwrap();
        assert!(json.contains("\"start\":true"));
        assert!(json.contains("\"type\":\"streaming\""));
    }

    #[test]
    fn test_capture_control_serialization() {
        let capture_cmd = CaptureControl {
            frequency_center: Some(1920e6),
            frequency_span: Some(200e6),
            frequency_start: None,
            frequency_end: None,
            frequency_bins: Some(448),
            reference_level: Some(-20.0),
            control_type: ControlType::Capture,
        };

        let json = serde_json::to_string(&capture_cmd).unwrap();
        assert!(json.contains("1920000000"));
        assert!(json.contains("\"type\":\"capture\""));
    }

    #[tokio::test]
    #[ignore] // Only run when explicitly requested with --ignored - requires RTSA Suite Pro at atc.local:54664
    async fn test_rtsa_suite_pro_endpoint_debugging() {
        let base_url = "http://atc.local:54664";
        let client = match HttpEndpointsClient::new(base_url.to_string(), AuthMethod::None) {
            Ok(client) => client,
            Err(e) => {
                println!("❌ Failed to create HTTP client: {}", e);
                return;
            }
        };

        println!("🔍 RTSA Suite Pro HTTP Endpoint Debugging");
        println!("   Target: {}", base_url);
        println!("   Purpose: Analyze endpoint responses for licensing detection");
        println!();

        // Test basic connectivity first
        println!("📡 1. Testing Basic Connectivity");
        match client.test_connection().await {
            Ok(_) => println!("   ✅ Connection successful"),
            Err(e) => {
                println!("   ❌ Connection failed: {}", e);
                println!("   ⚠️  Cannot continue with endpoint testing");
                return;
            }
        }
        println!();

        // Test /info endpoint (should always work)
        println!("📋 2. Testing /info Endpoint (License-Free)");
        match client.get_info().await {
            Ok(info) => {
                println!("   ✅ Server Info Retrieved:");
                println!("      Name: {}", info.name);
                println!("      Title: {}", info.title);
                println!("      UUID: {}", info.uuid);
                println!("      Port: {}", info.port);
                println!("      Mission: {}", info.mission);
            }
            Err(e) => {
                println!("   ❌ Server Info Failed: {}", e);
                println!(
                    "      Status Analysis: {}",
                    analyze_http_error(&e.to_string())
                );
            }
        }
        println!();

        // Test /healthstatus endpoint (should always work)
        println!("🏥 3. Testing /healthstatus Endpoint (License-Free)");

        // First, let's get the raw response to see what's actually returned
        let health_url = format!("{}/healthstatus", base_url);
        match reqwest::get(&health_url).await {
            Ok(response) => {
                if response.status().is_success() {
                    match response.text().await {
                        Ok(raw_text) => {
                            println!("   📋 Raw healthstatus response (first 500 chars):");
                            let preview = preview(&raw_text, 500);
                            println!("      {}", preview);

                            // Try to parse as JSON to see the structure
                            match serde_json::from_str::<serde_json::Value>(&raw_text) {
                                Ok(json) => {
                                    println!("   📊 JSON structure detected:");
                                    if let Some(obj) = json.as_object() {
                                        for (key, _) in obj.iter().take(5) {
                                            println!("      - {}", key);
                                        }
                                        if obj.len() > 5 {
                                            println!("      ... and {} more fields", obj.len() - 5);
                                        }
                                    }
                                }
                                Err(e) => {
                                    println!("   ❌ JSON parsing failed: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            println!("   ❌ Failed to read response text: {}", e);
                        }
                    }
                } else {
                    println!("   ❌ HTTP request failed: {}", response.status());
                }
            }
            Err(e) => {
                println!("   ❌ Request failed: {}", e);
            }
        }

        // Now try the structured parsing
        match client.get_health_status().await {
            Ok(health) => {
                println!("   ✅ Health Status Retrieved Successfully:");
                match &health {
                    ConfigItem::Group {
                        name,
                        label,
                        items,
                        flags,
                    } => {
                        println!("      Type: Group");
                        println!("      Name: {}", name);
                        println!("      Label: {}", label);
                        println!("      Flags: {}", flags);
                        println!("      Items: {}", items.len());
                    }
                    _ => {
                        println!("      Type: Non-group item");
                    }
                }
            }
            Err(e) => {
                println!("   ❌ Structured parsing failed: {}", e);
                println!(
                    "      Status Analysis: {}",
                    analyze_http_error(&e.to_string())
                );
            }
        }
        println!();

        // Test /inputs endpoint (should always work)
        println!("🔌 4. Testing /inputs Endpoint (License-Free)");
        match client.get_inputs().await {
            Ok(inputs) => {
                println!("   ✅ Inputs Retrieved: {} available", inputs.len());
                for (i, input) in inputs.iter().enumerate().take(3) {
                    println!("      Input {}: {}", i + 1, input);
                }
                if inputs.len() > 3 {
                    println!("      ... and {} more", inputs.len() - 3);
                }
            }
            Err(e) => {
                println!("   ❌ Inputs Failed: {}", e);
                println!(
                    "      Status Analysis: {}",
                    analyze_http_error(&e.to_string())
                );
            }
        }
        println!();

        // Test /remoteconfig endpoint (THIS IS THE KEY TEST)
        println!("🔧 5. Testing Remote Config License Detection");
        println!("   ⚠️  USER CONFIRMED: No Remote Config license active");
        println!("   🔍 Investigating why we get responses anyway...");

        // First, let's get the raw response to understand what we're actually getting
        let config_url = format!("{}/remoteconfig", base_url);
        match reqwest::get(&config_url).await {
            Ok(response) => {
                println!("   📊 Raw /remoteconfig response:");
                println!("      Status: {}", response.status());
                println!(
                    "      Headers: {:?}",
                    response.headers().get("content-type")
                );

                if response.status().is_success() {
                    match response.text().await {
                        Ok(raw_text) => {
                            println!("   📋 Raw response (first 800 chars):");
                            let preview = preview(&raw_text, 800);
                            println!("      {}", preview);

                            // Try to parse as JSON
                            match serde_json::from_str::<serde_json::Value>(&raw_text) {
                                Ok(json) => {
                                    println!("   🔍 Analyzing JSON structure:");
                                    if let Some(obj) = json.as_object() {
                                        for (key, value) in obj.iter() {
                                            match value {
                                                serde_json::Value::Object(inner) => {
                                                    println!(
                                                        "      - {}: Object with {} fields",
                                                        key,
                                                        inner.len()
                                                    );
                                                }
                                                serde_json::Value::Array(arr) => {
                                                    println!(
                                                        "      - {}: Array with {} items",
                                                        key,
                                                        arr.len()
                                                    );
                                                }
                                                _ => {
                                                    println!("      - {}: {}", key, value);
                                                }
                                            }
                                        }
                                    }

                                    // Check if this looks like a limited/demo response
                                    if let Some(config_obj) = json.get("config") {
                                        println!("   🎯 Config object analysis:");
                                        if let Some(config_map) = config_obj.as_object() {
                                            println!(
                                                "      Config type: {:?}",
                                                config_map.get("type")
                                            );
                                            println!(
                                                "      Config name: {:?}",
                                                config_map.get("name")
                                            );
                                            if let Some(items_array) =
                                                config_map.get("items").and_then(|i| i.as_array())
                                            {
                                                println!(
                                                    "      Config items: {} available",
                                                    items_array.len()
                                                );
                                                for (i, item) in items_array.iter().enumerate() {
                                                    if let Some(name) =
                                                        item.get("name").and_then(|n| n.as_str())
                                                    {
                                                        println!("        [{}] {}", i, name);
                                                    }
                                                }
                                                if items_array.is_empty() {
                                                    println!(
                                                        "      ⚠️  WARNING: No config items - likely limited access!"
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    println!("   ❌ JSON parsing failed: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            println!("   ❌ Failed to read response text: {}", e);
                        }
                    }
                } else {
                    println!(
                        "   🎯 Non-success status: {} - This is more expected for unlicensed!",
                        response.status()
                    );
                }
            }
            Err(e) => {
                println!("   ❌ Request failed: {}", e);
            }
        }

        // Now test our detection method
        let license_status = client.probe_remote_config_write_license().await;
        println!();
        println!(
            "   🎯 OUR DETECTION RESULT: {}",
            license_status.description().to_uppercase()
        );
        println!("   ⚠️  USER SAYS: No license active");
        println!("   🤔 DISCREPANCY: Need to improve detection logic!");

        match license_status {
            RemoteConfigStatus::Active => {
                println!("   ❌ DETECTION ERROR: We detected active, but user says no license");
                println!("      💡 This suggests we need better detection criteria");

                // Get the config to analyze what we're actually getting
                if let Ok(config) = client.get_config().await {
                    println!("      📊 Config Analysis:");
                    println!("         Request ID: {}", config.request);
                    analyze_config_structure(&config.config, 0);

                    // Let's count actual configurable items
                    let item_count = count_configurable_items(&config.config);
                    println!("      📈 Configurable items found: {}", item_count);

                    if item_count == 0 {
                        println!("      🎯 REVISED DETECTION: No configurable items = NO LICENSE");
                    } else {
                        println!(
                            "      🤷 UNCLEAR: Found {} items but user says no license",
                            item_count
                        );
                    }
                }
            }
            _ => {
                println!("   ✅ Detection correctly identified no license");
            }
        }
        println!();

        // Test /user endpoint for authentication info
        println!("👤 6. Testing /user Endpoint (Authentication Info)");
        match client.get_user().await {
            Ok(user) => {
                println!("   ✅ User Info Retrieved:");
                println!("      Name: {}", user.name);
                println!("      Email: {:?}", user.email);
                println!("      Groups: {:?}", user.groups);
                println!("      Token Length: {}", user.token.len());
            }
            Err(e) => {
                println!("   ❌ User Info Failed: {}", e);
                println!(
                    "      Status Analysis: {}",
                    analyze_http_error(&e.to_string())
                );
            }
        }
        println!();

        // Summary
        println!("📊 ENDPOINT DEBUGGING SUMMARY");
        println!("   This test helps identify:");
        println!("   • Which endpoints require licensing");
        println!("   • What error codes indicate missing licenses");
        println!("   • Whether configuration changes can be verified");
        println!("   • Authentication and permission structures");
        println!();
        println!("✅ HTTP Endpoint Debugging Completed!");
    }

    #[tokio::test]
    #[ignore] // Only run when explicitly requested with --ignored - requires RTSA Suite Pro at atc.local:54664
    async fn test_remote_config_license_detection() {
        let base_url = "http://atc.local:54664";
        let client = match HttpEndpointsClient::new(base_url.to_string(), AuthMethod::None) {
            Ok(client) => client,
            Err(e) => {
                println!("❌ Failed to create HTTP client: {}", e);
                return;
            }
        };

        println!("🔍 Remote Config License Detection Test");
        println!("   Target: {}", base_url);
        println!();

        // Test the automatic license detection
        println!("🎯 1. Automatic License Detection");
        let status = client.probe_remote_config_write_license().await;
        println!("   Status: {:?}", status);
        println!("   Description: {}", status.description());
        println!("   Available: {}", status.is_available());
        println!();

        // If license is active, test configuration change verification
        if status.is_available() {
            println!("🔄 2. Configuration Change Verification");
            println!("   Testing read-modify-write cycle...");

            match client.verify_config_changes("main/reflevel", 0.5).await {
                Ok(verification) => {
                    println!("   ✅ Verification Results:");
                    println!("      Parameter: {}", verification.parameter);
                    println!("      Original Value: {} dBm", verification.original_value);
                    println!("      Test Value: {} dBm", verification.test_value);
                    println!("      Final Value: {} dBm", verification.current_value);
                    println!("      Change Applied: {}", verification.applied);

                    if let Some(error) = &verification.error {
                        println!("      Error: {}", error);
                    }

                    // Validate that the test worked as expected
                    if verification.applied {
                        println!("   🎯 RESULT: Configuration changes ARE being applied");
                    } else {
                        println!("   ⚠️  RESULT: Configuration changes are NOT being applied");
                    }
                }
                Err(e) => {
                    println!("   ❌ Verification failed: {}", e);
                }
            }
        } else {
            println!("🔄 2. Configuration Change Verification");
            println!("   ⏭️  Skipped - License not available");
            println!(
                "   Recommendation: Activate Remote Config license to test configuration changes"
            );
        }

        println!();
        println!("✅ License Detection Test Completed!");
    }

    #[test]
    fn test_remote_config_status_enum() {
        // Test the RemoteConfigStatus enum functionality
        let active = RemoteConfigStatus::Active;
        let not_licensed = RemoteConfigStatus::NotLicensed;
        let auth_required = RemoteConfigStatus::AuthenticationRequired;
        let unknown = RemoteConfigStatus::Unknown("test error".to_string());

        // Test is_available() method
        assert!(active.is_available());
        assert!(!not_licensed.is_available());
        assert!(!auth_required.is_available());
        assert!(!unknown.is_available());

        // Test description() method
        assert_eq!(active.description(), "Remote Config license is active");
        assert_eq!(
            not_licensed.description(),
            "Remote Config license not available"
        );
        assert_eq!(auth_required.description(), "Authentication required");
        assert_eq!(unknown.description(), "Unknown status");

        // Test PartialEq
        assert_eq!(active, RemoteConfigStatus::Active);
        assert_eq!(not_licensed, RemoteConfigStatus::NotLicensed);
    }

    #[test]
    fn test_config_change_verification_struct() {
        let verification = ConfigChangeVerification {
            applied: true,
            parameter: "main/reflevel".to_string(),
            original_value: -30.0,
            test_value: -29.0,
            current_value: -29.0,
            error: None,
        };

        assert!(verification.applied);
        assert_eq!(verification.parameter, "main/reflevel");
        assert_eq!(verification.original_value, -30.0);
        assert_eq!(verification.test_value, -29.0);
        assert_eq!(verification.current_value, -29.0);
        assert!(verification.error.is_none());
    }

    // Helper function to analyze HTTP error messages
    fn analyze_http_error(error_msg: &str) -> String {
        let msg = error_msg.to_lowercase();

        if msg.contains("403") || msg.contains("forbidden") {
            "403 Forbidden - Likely license/permission issue".to_string()
        } else if msg.contains("401") || msg.contains("unauthorized") {
            "401 Unauthorized - Authentication required".to_string()
        } else if msg.contains("402") || msg.contains("payment") {
            "402 Payment Required - License/payment needed".to_string()
        } else if msg.contains("404") || msg.contains("not found") {
            "404 Not Found - Endpoint not available".to_string()
        } else if msg.contains("500") || msg.contains("internal server") {
            "500 Internal Server Error - Server-side issue".to_string()
        } else if msg.contains("timeout") || msg.contains("connection") {
            "Network/Connection Issue".to_string()
        } else {
            format!("Other Error: {}", error_msg)
        }
    }

    // Helper function to count actual configurable items (not just groups)
    fn count_configurable_items(config: &ConfigItem) -> usize {
        match config {
            ConfigItem::Group { items, .. } => items.iter().map(count_configurable_items).sum(),
            ConfigItem::Number { .. }
            | ConfigItem::Float { .. }
            | ConfigItem::Bool { .. }
            | ConfigItem::String { .. } => 1,
            _ => 0,
        }
    }

    // Helper function to analyze configuration structure
    fn analyze_config_structure(config: &ConfigItem, depth: usize) {
        let indent = "  ".repeat(depth + 2);

        match config {
            ConfigItem::Group {
                name, label, items, ..
            } => {
                if depth < 2 {
                    // Limit depth to avoid too much output
                    println!("{}📁 Group: {} ({})", indent, name, label);
                    println!("{}   Items: {}", indent, items.len());

                    // Show a few key items
                    for item in items.iter().take(3) {
                        if let ConfigItem::Number {
                            name, value, unit, ..
                        } = item
                        {
                            println!("{}   📊 {}: {} {:?}", indent, name, value, unit);
                        } else if let ConfigItem::Float {
                            name, value, unit, ..
                        } = item
                        {
                            println!("{}   📊 {}: {} {:?}", indent, name, value, unit);
                        }
                    }
                    if items.len() > 3 {
                        println!("{}   ... and {} more items", indent, items.len() - 3);
                    }
                }
            }
            ConfigItem::Number {
                name, value, unit, ..
            } => {
                if depth == 0 {
                    println!("{}📊 Number: {} = {} {:?}", indent, name, value, unit);
                }
            }
            ConfigItem::Float {
                name, value, unit, ..
            } => {
                if depth == 0 {
                    println!("{}📊 Float: {} = {} {:?}", indent, name, value, unit);
                }
            }
            _ => {
                if depth == 0 {
                    println!("{}📋 Other config item type", indent);
                }
            }
        }
    }

    /// Verifies the recursive config-tree walk (A16): a block nested
    /// inside a scene group must still be found. The top-level-only
    /// walk in earlier versions of this client would silently return
    /// "not found" on this shape.
    #[test]
    fn test_find_config_item_recursive() {
        let tree = serde_json::json!([
            { "name": "Block_Filter_0" },
            {
                "name": "Scene_Group_A",
                "items": [
                    { "name": "Block_Spectrum_0" },
                    {
                        "name": "Inner_Group",
                        "items": [
                            { "name": "Block_IQDemodulator_3" },
                        ],
                    },
                ],
            },
        ]);
        let items = tree.as_array().unwrap();
        assert_eq!(
            find_config_item_by_name_prefix(items, "Block_IQDemodulator"),
            Some("Block_IQDemodulator_3".to_string()),
            "recursive walker should find the IQ demodulator block nested two levels deep"
        );
        assert_eq!(
            find_config_item_by_name_prefix(items, "Block_Filter"),
            Some("Block_Filter_0".to_string()),
            "top-level matches must still work"
        );
        assert_eq!(
            find_config_item_by_name_prefix(items, "Block_Nonexistent"),
            None,
            "missing prefix yields None"
        );
    }

    /// Pre-order property: when the prefix matches at multiple
    /// depths, the shallowest (closest to root) match wins.
    #[test]
    fn test_find_config_item_prefers_shallow_match() {
        let tree = serde_json::json!([
            { "name": "Block_IQDemodulator_0" },
            {
                "name": "Group",
                "items": [
                    { "name": "Block_IQDemodulator_5" },
                ],
            },
        ]);
        let items = tree.as_array().unwrap();
        assert_eq!(
            find_config_item_by_name_prefix(items, "Block_IQDemodulator"),
            Some("Block_IQDemodulator_0".to_string()),
            "pre-order traversal should yield the root-level match first"
        );
    }

    /// A synthetic `/remoteconfig` `config.items` array shaped like a live
    /// SPECTRAN V6 ECO: the tuner is a `spectrumanalyzer` block with the
    /// fields nested `Block/config/main`, and a decoy block without
    /// `centerfreq0` sits ahead of it. Mirrors the tree captured from
    /// `atc.local` down to the `decimation0` enum labels.
    fn v6_config_items() -> serde_json::Value {
        serde_json::json!([
            {
                "type": "group",
                "name": "Block_HttpServer_0",
                "items": [
                    { "type": "group", "name": "config", "items": [
                        { "type": "bool", "name": "run", "value": true }
                    ]}
                ]
            },
            {
                "type": "group",
                "name": "Block_Spectran_V6Eco_0",
                "items": [
                    { "type": "group", "name": "config", "items": [
                        { "type": "group", "name": "main", "items": [
                            { "type": "float", "name": "centerfreq0", "value": 851_656_250.0 },
                            { "type": "enum",  "name": "decimation0", "value": 5,
                              "values": "Full,1 / 2,1 / 4,1 / 8,1 / 16,1 / 32,1 / 64,1 / 128,1 / 256,1 / 512" },
                            { "type": "float", "name": "reflevel0",  "value": -25.0 }
                        ]}
                    ]}
                ]
            }
        ])
    }

    #[test]
    fn discovers_block_carrying_centerfreq0() {
        let tree = v6_config_items();
        let items = tree.as_array().unwrap();
        assert_eq!(
            find_block_name_carrying_field(items, "centerfreq0", None),
            Some("Block_Spectran_V6Eco_0".to_string()),
            "discovery must skip the decoy block and find the tuner by its field",
        );
        // The legacy prefix scan finds nothing on this device — the reason
        // the retune silently no-opped before.
        assert_eq!(
            find_config_item_by_name_prefix(items, "Block_IQDemodulator"),
            None,
            "the old IQ-demodulator prefix cannot match a spectrumanalyzer block",
        );
    }

    #[test]
    fn discovery_returns_none_for_absent_field() {
        let tree = v6_config_items();
        let items = tree.as_array().unwrap();
        assert_eq!(
            find_block_name_carrying_field(items, "nonesuch", None),
            None
        );
    }

    #[test]
    fn discovery_ignores_field_outside_any_block() {
        // A leaf above any `Block_*` group cannot be a receiverName, so it
        // must not be returned even though the name matches.
        let tree = serde_json::json!([
            { "type": "float", "name": "centerfreq0", "value": 1.0 },
            { "type": "group", "name": "Scene", "items": [
                { "type": "float", "name": "centerfreq0", "value": 2.0 }
            ]}
        ]);
        let items = tree.as_array().unwrap();
        assert_eq!(
            find_block_name_carrying_field(items, "centerfreq0", None),
            None
        );
    }

    #[test]
    fn reads_capture_leaves_from_block_subtree() {
        let tree = v6_config_items();
        let items = tree.as_array().unwrap();
        let block_items = find_group_items(items, "Block_Spectran_V6Eco_0").unwrap();
        assert_eq!(
            read_config_leaf_value(block_items, "centerfreq0"),
            Some(851_656_250.0)
        );
        // An enum leaf reads back as its index.
        assert_eq!(
            read_config_leaf_value(block_items, "decimation0"),
            Some(5.0)
        );
        assert_eq!(
            read_config_leaf_value(block_items, "reflevel0"),
            Some(-25.0)
        );
        assert_eq!(read_config_leaf_value(block_items, "nonesuch"), None);
    }
}
