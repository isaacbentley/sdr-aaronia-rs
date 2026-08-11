# Aaronia RTSA HTTP Streaming Protocol Specification

## Overview

The Aaronia Real-Time Spectrum Analyzer (RTSA) HTTP streaming protocol provides real-time access to measurement data through REST API endpoints. This specification covers the complete HTTP streaming protocol, data formats, and implementation guidelines for high-performance RF data streaming.

> **Status & attribution.** This document is a *community-compiled* reference, **not** an official Aaronia specification. It is assembled from public posts on the Aaronia V6 forum, Aaronia's product documentation, and empirical analysis. Where these disagree, the vendor's own materials are authoritative. See [Sources and Attribution](#sources-and-attribution) for the upstream, vendor-published references.

## Table of Contents

- [Stream Format Types](#stream-format-types)
- [Packet Structure](#packet-structure)
- [Data Formats](#data-formats)
- [HTTP Endpoints](#http-endpoints)
- [Payload Types](#payload-types)
- [Authorization & Licensing](#authorization--licensing)
- [Performance Optimization](#performance-optimization)
- [Implementation Guidelines](#implementation-guidelines)
- [Revision History](#revision-history)
- [Sources and Attribution](#sources-and-attribution)

## Features and Purpose

The HTTP stream server (and client) block are used to stream measurement and detection data from and to the RTSA suite. It is also used to control and monitor the streaming. The protocol uses HTTP as the underlying transport protocol and the REST paradigm for the API.

## Block Graph

An HTTP server block in the block graph of the RTSA suite provides the access. More than one HTTP server block may be present in one graph, but will have to use different ports for their listening socket. The data streamed will depend on the graph used in the RTSA suite.

### Typical Data Flow Configurations

1. **Basic HTTP Server Block**: RF input → Spectran V6B → IQ/Spectra streams → HTTP Server
2. **Power Spectrum Block**: RF input → Spectran V6B → IQ Power Spectrum → HTTP Server
3. **Demodulation Block**: RF input → Spectran V6B → IQ Demodulator → IQ Power Spectrum → HTTP Server
4. **Sweep Block**: IQ Power Spectrum → Spectrum Sweep → Chain → Stream → HTTP Server
5. **Condition Block**: IQ Power Spectrum → Spectrum Condition (≥X) → Filtered Spectra → HTTP Server

## Stream Format Types

```rust
enum StreamFormat {
    Json,       // Human-readable JSON (slow)
    Int16,      // 16-bit signed integers with scale factor
    Float16,    // Half precision IEEE 754
    Float32,    // Full precision floating point
}
```

## Packet Structure

HTTP streaming uses a JSON metadata line followed by binary sample data. **Verified against live SpectranV6 hardware** (RTSA-Suite PRO HTTP server block, float32/int16, iq and spectra payloads): the JSON line is terminated by a line feed (0x0A) and the binary section is prefixed by an ASCII Record Separator (0x1E) — i.e. **two** separator bytes between JSON and binary:

```
{JSON_METADATA}[0x0A][0x1E][BINARY_SAMPLE_DATA]
```

Earlier revisions of this document (and the upstream description "separated by RS or LF") implied a single separator byte; parsing with a single separator shifts every binary sample by one byte. The crate's parser accepts both the two-byte form and a lone LF/RS.

## Data Formats

The stream server supports multiple data formats for high-performance streaming:

1. **Pure JSON**: Human-readable but slower
2. **Combined JSON + Binary**: JSON metadata followed by binary sample data
3. **Raw Binary**: Maximum performance for high data rates

### Binary Data Encoding

| Format | IQ Encoding | Bytes per Sample |
|---------|-------------|------------------|
| Float32 | I,Q as IEEE 754 | 8 (4+4) |
| Float16 | I,Q as half precision | 4 (2+2) |
| Int16 | I,Q as signed integers | 4 (2+2) |

### Complete Metadata Schema

```json
{
  "startTime": 1501163970.1396854,
  "endTime": 1501163970.140799,
  "startTimeDay": 17372,
  "endTimeDay": 17372,
  "startFrequency": 2400000000.0,
  "endFrequency": 2500000000.0,
  "sampleFrequency": 100000000.0,
  "samples": 1024,
  "unit": "dbm",
  "payload": "iq",
  "minPower": -120,
  "maxPower": 10,
  "sampleSize": 2,
  "sampleDepth": 1,
  "scale": 16384,
  "antenna": {
    "name": "Omni",
    "latitude": 52.520008,
    "longitude": 13.404954,
    "azimuth": 0.0,
    "declination": 0.0
  },
  "categories": [
    {
      "name": "WiFi Channel 1",
      "startFrequency": 2401000000,
      "endFrequency": 2423000000
    }
  ]
}
```

## General Fields (Common to All Stream Formats)

| Field | Description | Data Type |
| :--- | :--- | :--- |
| `startTime` | Start time of the packet in seconds since the Unix epoch | `double` (e.g., `1501163970.1396854`) |
| `endTime` | End time of the packet in seconds since the Unix epoch | `double` (e.g., `1501163970.140799`) |
| `startTimeDay` / `endTimeDay` | Day component of the packet timestamps, present in live-device headers | `integer` |
| `unit` | Unit of the sample values | `string` (e.g., `"dbm"`, `"generic"`, `"percentage"`) |
| `payload` | Payload type of the packet | `string` (e.g., `"spectra"`, `"iq"`, `"histogram"`) |
| `minPower` | Minimum power in dBm | `integer` (e.g., `-95`, `-2`, `-165`); this crate parses it as `i32` |
| `maxPower` | Maximum power in dBm | `integer` (e.g., `5`, `2`); this crate parses it as `i32` |
| `sampleFrequency` | Sample rate in Hz | `double` (e.g., `100000000.0`) |
| `compression` | Compression scheme of the binary payload; drives the wavelet decompression path when present | `integer` |
| `startFrequency` | Start of a frequency range | `double` (e.g., `2400250000`, `2402250128`) |
| `endFrequency` | End of a frequency range | `double` (e.g., `2487750000`, `2489750128`) |
| `sampleDepth` | Number of sample sets per sample, e.g., bins in a histogram | `integer` (e.g., `1`, `2`, `256`) |
| `sampleSize` | Sample size, e.g., individual frequency bins in a spectrum | `integer` (e.g., `448`, `896`) |
| `samples` | JSON format: array of actual sample data. Binary formats: **count** — IQ pairs for `iq`, or spectra **frames** for `spectra` (each frame is `sampleSize × sampleDepth` values; live-verified: `samples=64, sampleSize=820` ⇒ 64·820 float32 values in the payload) | `array` or `integer` |
| `antenna` | Antenna specification. Real servers may send only `{"name":""}` with no position fields | `object` |
| `categories` | Category specification | `array of objects` |
| `scale` | **Encode multiplier** for integer data: `int16 = round(value * scale)`, so decoding divides (`value = raw / scale`). Live-verified: a dBm spectra stream with `scale: 100` carries raw `-11378` = −113.78 dBm | `double` |

## HTTP Endpoints

### Stream Data Endpoint
**URL**: `/stream`  
**Method**: GET  
**Description**: Streaming with chunked transfer encoding. Data transmitted as line-limited JSON with packets separated by line feed (ASCII 10) and record separator (ASCII 30).

**Parameters**:
- `format`: Output format (`json`, `int16`, `float16`, `float32`)
- `limit`: Maximum number of **packets** to stream before the server closes the connection (live-verified: `?limit=N` delivers exactly N packets)
- `rate_reduction=n`: Reduce sample rate by factor of n
- `rate_adaption=0`: Disable automatic rate adaptation
- `scale`: Server-side scale factor for integer formats. **Distinct from
  the per-packet `scale` JSON metadata field** — the URL parameter scales
  the integer payload before transmission, e.g.
  `/stream?format=int16&scale=1000000`.
- `input`: Select different input than "main"

**Backpressure**: per the v9 endpoints PDF, "the RTSA HTTP server block
will start dropping data when the outbound TCP buffer exceeds 8 Mbytes.
A loss of data can be determined by comparing the timestamps of two
adjacent data packets." The Rust binding exposes
`http_streaming::DropDetector` for that timestamp-gap check.

**Examples**:
```
http://localhost:54664/stream?format=float32&limit=1000
http://localhost:54664/stream?format=int16&scale=1000000
http://localhost:54664/stream?rate_reduction=10
```

### Application Process Control
**URL**: `/app/process`  
**Method**: PUT  
**Description**: Gracefully shut down the RTSA-Suite application. **This
targets the RTSA application's HTTP control surface — *not* the HTTP
server block embedded in a mission graph.** Both expose REST endpoints
on different ports, and sending this PUT to a mission's HTTP server
block has no effect.

**Body**:
```json
{ "running": false }
```

The Rust binding exposes this via
`HttpEndpointsClient::shutdown_application`. Source: official forum
thread "Close RTSA application with an api" (resolved 2025-05-14).

### Single Sample Endpoint
**URL**: `/sample`  
**Method**: GET  
**Parameters**: `input` — select an input other than `main` (optional)  
**Description**: Polling single samples from the server block input
**Response**: Contains one or no samples in JSON format

**Example Response**:
```json
{
  "startTime": 1509964956.39194,
  "endTime": 1509964956.394119,
  "startFrequency": 2402250128,
  "endFrequency": 2487750128,
  "unit": "dbm",
  "payload": "spectra",
  "minPower": -165,
  "maxPower": 5,
  "sampleSize": 448,
  "antenna": {
    "name": "IsoLOG 3D",
    "latitude": 50.13608551,
    "longitude": 6.3196878433,
    "azimuth": -2.748893976211548,
    "declination": 0
  },
  "samples": [
    [-113.25, -108.65, -106.83],
    [-98.81, -97.61, -128.91, -121.16]
  ]
}
```

Note: this crate's `get_sample()` deserializes the response as a full
packet-metadata object, so `payload`, `minPower`, `maxPower`, and
`sampleSize` must be present (they are in live-device responses).

### Multiple Samples Endpoint
**URL**: `/samples`  
**Method**: GET  
**Parameters**: `limit` — maximum number of samples to return; `input` — select an input other than `main`. Both optional.  
**Description**: Polling a batch of samples from the server block input. The response is a JSON array of the same packet-metadata objects returned by `/sample`. Exposed via `HttpEndpointsClient::get_samples`.

### Sample Push Endpoint (TX)
**URL**: `/sample`  
**Method**: POST  
**Content-Type**: application/json  
**Description**: Pushes IQ samples *to* the server block for transmission. This is the TX path used by `HttpEndpointsClient::push_samples` (and the `HttpSink` FutureSDR block).

**Payload** (camelCase; `samples` is a flat array of interleaved I/Q floats):
```json
{
  "startTime": 1509964956.39194,
  "endTime": 1509964956.394119,
  "startFrequency": 432500000.0,
  "endFrequency": 433500000.0,
  "stepFrequency": 1000000.0,
  "minPower": -1.0,
  "maxPower": 1.0,
  "sampleSize": 2,
  "sampleDepth": 1,
  "unit": "generic",
  "payload": "iq",
  "push": true,
  "samples": [0.01, -0.02, 0.03, 0.04]
}
```

`stepFrequency` (the sample rate) is optional; `push: true` marks the packet for immediate transmission.

### Control Endpoint
**URL**: `/control`  
**Method**: PUT  
**Description**: Send commands to the RTSA suite
**Content-Type**: application/json

#### Start/Stop Streaming
```json
{
  "start": true,
  "type": "streaming"
}
```

#### Set Frequency Range
```json
{
  "frequencyCenter": 1200000000,
  "frequencySpan": 44000000,
  "frequencyBins": 448,
  "referenceLevel": -20,
  "type": "capture"
}
```

**Alternative frequency specification**:
```json
{
  "frequencyStart": 75.0e6,
  "frequencyEnd": 6000.0e6,
  "type": "capture"
}
```

> **Both frequency fields are required for a retune to apply.** Live
> testing against RTSA-Suite PRO (HTTP server block fed by a SPECTRAN
> V6 ECO) shows the server returns `{"success":true}` for a capture
> `PUT` carrying only `frequencyCenter` or only `frequencySpan` but
> silently ignores it — the device keeps streaming at its previous
> tuning. Sending `frequencyCenter` **and** `frequencySpan` together
> applies reliably. `referenceLevel` on its own does apply. No license
> is involved: `/control` capture writes work without the Remote
> Config license.

#### Start/Stop Antenna Autorotation
```json
{
  "rotate": true,
  "type": "antenna"
}
```

#### Start/Stop Recording
```json
{
  "start": true,
  "filename": "recording_name",
  "type": "recording"
}
```

#### Save/Reload Mission
```json
{
  "save": true,
  "type": "mission"
}
```

### Configuration Endpoints

#### Server Info (`/info`)
**URL**: `/info`  
**Method**: GET  
**Response**:
```json
{
  "name": "Block_HTTPServer_0",
  "title": "HTTP Server",
  "uuid": "aaf2a8f7-11fa-45a3-bcfc-26aaf5957629",
  "port": 54664,
  "mission": ""
}
```

#### Available Inputs (`/inputs`)
**URL**: `/inputs`  
**Method**: GET  
**Response**:
```json
{
  "inputs": ["main", "{3b459e11-74e1-4b82-88ad-28459dfe2fe1}"]
}
```

**Method**: POST  
**Description**: Create new inputs based on existing ones
**Payload**:
```json
{
  "input": "main",
  "type": "average"
}
```

(Upstream forum material shows the first key capitalized as `"Input"`; this crate sends lowercase `"input"`, which is what its mock-server tests pin.)

**Available processing types**:
- `average`: Average of a series of samples
- `maxhold`: Maximum of a series of samples
- `minhold`: Minimum of a series of samples
- `maxfall`: Falling maximum of a series of samples
- `histogram`: Histogram of samples
- `waterfall`: Time compressed samples

#### Remote Configuration (`/remoteconfig`)
**URL**: `/remoteconfig`  
**Method**: GET - Query current configuration  
**Method**: PUT - Update configuration

**GET Response Structure**:
```json
{
  "request": 0,
  "config": {
    "type": "group",
    "name": "remoteconfig",
    "label": "RemoteConfig",
    "items": [
      {
        "type": "group",
        "name": "Block_FileReader_0",
        "label": "File Reader"
      }
    ]
  }
}
```

**Simplified PUT form**: in addition to the full `{request, config}` shape, the server accepts a shorter per-block write (used by the SDR++ and SDRangel Aaronia plugins), exposed via `HttpEndpointsClient::simple_remote_config`:
```json
{
  "receiverName": "Block_IQDemodulator_0",
  "simpleconfig": {
    "main": { "centerfreq": 100000000.0, "samplerate": 10000000.0, "spanfreq": 10000000.0 }
  }
}
```
`receiverName` must match a block in the running mission; `HttpEndpointsClient::find_iq_demodulator_block_name` auto-discovers it.

**Configuration Item Fields**:
| Field | Description |
| :--- | :--- |
| `type` | Type of config element |
| `name` | Machine-readable name |
| `label` | Human-readable name |
| `min`/`max` | Value range for numeric types |
| `step` | Distance between valid values |
| `unit` | Unit for numeric values |
| `values` | Enumeration options |
| `flags` | Additional configuration flags |

#### Health Status (`/healthstatus`)
**URL**: `/healthstatus`  
**Method**: GET  
**Description**: Block health status and performance statistics

Note: this crate's `get_health_status()` parses the response as a generic
configuration tree (`HealthStatus` is an alias for `ConfigItem`) rather
than the typed shape below, which describes the upstream block-health
fields.

**Health Status Fields**:
| Field | Description |
| :--- | :--- |
| `state` | `unknown`, `idle`, `booting`, `ready`, `starting`, `operational`, `running`, `warning`, `critical` |
| `error` | User-facing error description |
| `date` | Last update timestamp (seconds since epoch) |
| `name` | Internal block name |
| `category` | Block category |
| `title` | User-facing block title |
| `uuid` | Global unique identifier |

#### User Information (`/user`)
**URL**: `/user`  
**Method**: GET  
**Response Fields**:
| Field | Description |
| :--- | :--- |
| `name` | Current user name |
| `email` | Email address if available |
| `token` | Authorization token for RToken method |
| `groups` | Array of group names |

## Payload Types

### IQ Data
IQ Samples are transmitted as a flat array of alternating I and Q values.

```json
{
  "payload": "iq",
  "unit": "generic",
  "minPower": -2,
  "maxPower": 2,
  "sampleSize": 2,
  "samples": [
    5.12e-05, 0.00132,
    0.000885, 0.00124,
    0.000566, 0.000654,
    -0.000615, 2.35e-05
  ]
}
```

### Spectrum Data
```json
{
  "startTime": 1501163970.1396854,
  "endTime": 1501163970.140799,
  "unit": "dbm",
  "payload": "spectra",
  "startFrequency": 2400250000,
  "endFrequency": 2487750000,
  "minPower": -95,
  "maxPower": 5,
  "antenna": {
    "name": "Block ISOLOG 0",
    "latitude": 50.13646697998047,
    "longitude": 6.320250034332275,
    "azimuth": -2.748893976211548,
    "declination": 0
  },
  "sampleDepth": 1,
  "sampleSize": 448,
  "samples": [
    [-90.05, -90.05, -81.01],
    [-81.65, -78.05, -90.01]
  ]
}
```

### Histogram Data
Histogram data transfers percentages of bin usage. Sample size is like spectrum data, but sample depth separates the bins.

```json
{
  "startTime": 1506933004.0587604,
  "endTime": 1506933004.0911448,
  "payload": "histogram",
  "unit": "percentage",
  "startFrequency": 2402250128,
  "endFrequency": 2489750128,
  "maxPower": 5,
  "minPower": -165,
  "sampleDepth": 256,
  "sampleSize": 896,
  "samples": [0.074, 0.0787, 0.0893]
}
```

Note: unlike `spectra` (one nested array per frame), histogram samples are a flat number array; that is what this crate's JSON parser accepts.

### Channel Power / Category Data
The samples in a category ordered packet have one measurement per category.

```json
{
  "payload": "categories",
  "categories": [
    {
      "name": "Wifi Channel 1",
      "startFrequency": 2401000000,
      "endFrequency": 2423000000
    }
  ],
  "samples": [-45.2, -67.8, -52.1]
}
```

As with histograms, category samples are a flat number array (one value per category).

### Antenna Data
Data captured using antennas with location or directional information.

| Field | Description |
| :--- | :--- |
| `name` | Antenna name |
| `latitude` | Antenna latitude |
| `longitude` | Antenna longitude |
| `azimuth` | Azimuth of directional antenna |
| `declination` | Declination of directional antenna |

## Authorization & Licensing

### HTTP Streaming vs Remote Configuration

**Basic HTTP Streaming** (No additional license required):
- `/stream` - Real-time data streaming
- `/sample` - Sample retrieval
- `/info` - Device information
- `/healthstatus` - Device health monitoring

**Remote Configuration** (Requires separate license):
- `/remoteconfig` - Device parameter configuration
- **License Required**: "Remote Config" license from Aaronia
- **Alternative**: Use Native SDK for configuration without HTTP licensing restrictions

**License Detection Methods**:

**Important**: Read access to `/remoteconfig` is available WITHOUT license, but write operations require the license. Because reads are license-free, a read-only check cannot distinguish "licensed" from "unlicensed" — only a write test can.

The client therefore exposes two methods:

- `detect_remote_config_license()` — **read-only**. Never touches device state. Classifies 401/403 responses; on read success it returns `Unknown` (write capability unproven).
- `probe_remote_config_write_license()` — **active probe**. Performs a read-modify-restore cycle on `reflevel` (+1 dB, restored best-effort) to positively verify write capability. Use only when you genuinely need proof of `/remoteconfig` write access. Frequency hopping does **not** need it — retuning goes through the license-free `/control` endpoint (see the capture-control note above; the silent-ignore behavior once attributed to licensing was traced to partial capture payloads).

**Practical Detection (Active Probe)**:
```rust
use sdr_aaronia_rs::http_endpoints::RemoteConfigStatus;

// NOTE: temporarily perturbs the device reference level by +1 dB.
match client.probe_remote_config_write_license().await {
    RemoteConfigStatus::Active => {
        // License ACTIVE - write operations work
        println!("Remote Config License: ACTIVE");
        // Full configuration control available
    },
    RemoteConfigStatus::NotLicensed => {
        // License NOT ACTIVE - read-only access
        println!("Remote Config License: NOT LICENSED");
        // Can read config but cannot make changes
    },
    RemoteConfigStatus::AuthenticationRequired => {
        println!("Authentication required");
    },
    RemoteConfigStatus::Unknown(err) => {
        println!("Detection failed: {}", err);
    },
    // `RemoteConfigStatus` is #[non_exhaustive]; a wildcard arm is required.
    status => println!("Unrecognized status: {status:?}"),
}
```

**Technical Details**:
- Detection uses `verify_config_changes()` with safe parameter testing
- Tests actual write capability, not just HTTP response codes
- Automatically restores original values after testing
- Provides definitive licensing status for production use

See: https://aaronia.com/en/software-licence-remote-config
Documentation: https://rtsa-manual.aaronia.com/en/Content/C_Operation/DDCommandCenter/RemoteConfig.htm

### Authentication Methods

The server supports two types of HTTP authorization:
- **Basic**: Username and password authentication
- **RToken**: User-specific token returned by `/user` endpoint

## Performance Optimization

### High Data Rate Considerations
- Use binary formats (`int16`, `float16`, `float32`) for maximum throughput
- TCP loopback fast path on Windows: custom clients can enable the `SIO_LOOPBACK_FAST_PATH` socket option (an OS-level tweak; not set by this crate)
- Buffer management with chunked transfer encoding
- Automatic rate adaptation available via `rate_adaption` parameter

### Format Selection Guidelines
- **JSON**: Development and debugging, low data rates
- **Int16**: High data rates with acceptable quantization
- **Float16**: Balanced precision and performance
- **Float32**: Maximum precision for critical applications

### Network Optimization
While achieving full 250M samples/sec over gigabit Ethernet is challenging with pure JSON, binary formats with fast parsers can approach line rate over loopback connections.

## Implementation Guidelines

### Stream Processing

```rust
// Process HTTP streaming data
let mut parser = StreamParser::new(StreamFormat::Float32, None)?;

// Handle chunked transfer encoding transparently
for http_chunk in http_response_chunks {
    let packets = parser.process_data(&http_chunk)?;
    for packet in packets {
        process_samples(&packet.samples);
        log_sdr_config(&packet.sdr_config);
    }
}
```

### Sample Parsing

```rust
// Float32 IQ samples (most common)
fn parse_iq_float32(data: &[u8]) -> Vec<Complex32> {
    data.chunks_exact(8)
        .map(|chunk| {
            let i = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let q = f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
            Complex32::new(i, q)
        })
        .collect()
}

// Int16 with scaling. The metadata `scale` field is the ENCODE multiplier
// (int16 = round(value * scale)), so decoding must invert it first —
// applying `scale` directly is a classic bug.
fn parse_iq_int16(data: &[u8], metadata_scale: f32) -> Vec<Complex32> {
    let decode_scale = 1.0 / metadata_scale;
    data.chunks_exact(4)
        .map(|chunk| {
            let i = i16::from_le_bytes([chunk[0], chunk[1]]) as f32 * decode_scale;
            let q = i16::from_le_bytes([chunk[2], chunk[3]]) as f32 * decode_scale;
            Complex32::new(i, q)
        })
        .collect()
}
```

### Error Handling

- Handle HTTP connection failures gracefully
- Implement retry logic for network interruptions  
- Validate JSON metadata before processing binary data
- Handle partial packets in streaming scenarios

### Memory Management

- Use buffer pools for high-throughput scenarios
- Implement zero-copy parsing where possible
- Consider async I/O for concurrent stream processing
- Monitor memory usage during long-running streams

---

## Revision History

| Version | Date | Changes |
|---------|------|---------|
| 1.0 | 2025-01-11 | Initial HTTP specification from original documentation |
| 2.0 | 2025-01-11 | Enhanced with comprehensive streaming protocol specification and implementation guidelines |
| 2.1 | 2026-08-06 | Live-hardware corrections folded in (two-byte separator, spectra frame counting, `scale` inversion); documented `/samples`, the `/sample` TX push, and the `simpleconfig` PUT form; corrected `limit` semantics, field types, and flat histogram/categories sample arrays; removed decorative icons |

---

## Sources and Attribution

This specification is a compiled, community-maintained document. It is **not**
published or endorsed by Aaronia AG, and it may lag or diverge from the
vendor's own materials. For authoritative, vendor-published references,
consult:

- **RTSA-Suite PRO HTTP streaming** (Aaronia V6 forum) — [v6-forum.aaronia.de/forum/topic/rtsa-suite-pro-http-streaming](https://v6-forum.aaronia.de/forum/topic/rtsa-suite-pro-http-streaming/)
- Aaronia RTSA-Suite PRO product documentation — <https://rtsa-manual.aaronia.com/>

The content here is derived from the above plus empirical analysis of live
RTSA-Suite PRO streams. "Aaronia", "RTSA", and "Spectran" are the property of
Aaronia AG.

---

*This specification covers the complete Aaronia RTSA HTTP streaming protocol for real-time RF data access. For file format specifications, see FILESPEC.md.*