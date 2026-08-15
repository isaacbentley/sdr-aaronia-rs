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

### An unknown `format` is not an error

`format=` accepts `json`, `int16`, `float16` and `float32`. `raw16` is
an accepted alias for `int16` — Aaronia's own Qt reference client
defaults to it — and produces byte-identical framing.

Anything the server does not recognise serves **the RTSA file format**
instead: a `DSFH` header followed by `STRM`/`ANTA` chunks, with HTTP
200 and no warning. A typo in `format=` therefore yields a completely
different wire format rather than an error, and a parser expecting
JSON-plus-binary will fail somewhere well past the point that would
have identified the cause. Verified against a live server: `format=`
with a nonsense value returned `DSFH`.

This crate builds the string from an enum, so it cannot typo, and the
SoapySDR plugin now warns on an unrecognised `format=` device argument
rather than silently falling back to the default.

### When the server drops data

The HTTP server block starts dropping data once its outbound TCP
buffer passes **8 MB**. Nothing announces it; the loss shows up as a
gap between the timestamps of two adjacent packets, which is what
`DropDetector` watches for. A slow consumer, a slow link, or a rate
the network cannot carry all end here, so reducing the wire format
(`format=int16`) or the rate (`rate_reduction=n`) is the fix rather
than a larger client-side buffer.

### Liveness

There is no status endpoint. Aaronia's own remote control notes probe
with `curl -v http://127.0.0.1:54664/api/status` and treat the `404 Not
Found` as the answer: a reply of any kind proves the port is open, the
mission is loaded and the HTTP Server block is running. A connection
refused or a timeout means it is not. `/info` serves the same purpose
and returns something useful, which is what this crate uses.

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

`/control` accepts **PUT only**. A GET — which is what a browser does
with the URL — is not supported.

**A command reaches every block that understands it, unless you scope
it.** Aaronia's 2022 specification says control commands "are not
addressed to a specific RTSA block and will be processed from all RTSA
blocks in the block graph", which is true as a default. Their support
staff refined it in 2024: `receiverUUID` and `receiverName` "can be
used to limit the requested setting to individual blocks", and "in some
cases it may be necessary to specify them". Read the names or UUIDs
from `/remoteconfig` first if you need to target one block.

These are one-off requests. Aaronia's own words: if they cannot be
executed, or conflict with the local configuration, the behaviour is
undefined — nothing reports the conflict. That is not theoretical: the
same device has been measured both honouring and ignoring an identical
full-tuple capture command in different mission states, answering
`success=true` both times. A retune that must be provable goes through
`/remoteconfig` and reads the value back
(`HttpEndpointsClient::apply_capture_config` does exactly this).

**The settings each `type` accepts**, per Aaronia support (January
2024), which goes well beyond the published specification:

| `type` | Settings |
| :--- | :--- |
| `capture` | `receiverUUID`, `receiverName`, `frequencyCenter`, `frequencyStart`, `frequencyBins`, `referenceLevel`, `start` — plus `frequencySpan` and `frequencyEnd`, which the specification documents and this crate uses |
| `deviceconnect` | `receiverUUID`, `receiverName`, `start` — connect or disconnect a device |
| `streaming` | `receiverUUID`, `receiverName`, `filename`, `start` |
| `recording` | `receiverUUID`, `receiverName`, `filename`, `start` |
| `mission` | `save`, `reload`, `load` (with the path in `file`) |
| `antenna` | `receiverUUID`, `receiverName`, `latitude`+`longitude` (both or neither), `azimuth`+`declination`, `rotate` |
| `camera` | `receiverUUID`, `receiverName`, `latitude`+`longitude`, `azimuth`+`declination`, `aperture`, `channel`, `altitude`, `start` |

`start`, `rotate`, `save`, `load` and `reload` are booleans. A
`filename` must be an absolute path **on the machine running
RTSA-Suite**, with forward slashes.

Zones cannot be configured this way; Aaronia state that remote
configuration of zones is not supported.
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

> **`frequencySpan` is a usable-bandwidth request, not a sample rate.**
> The device halves its top rate down a ladder — 61.44 MHz on the V6
> ECO this was measured against. The usable RF bandwidth it declares in
> `startFrequency..endFrequency` is exactly 0.8 x Fs at every rate,
> checked at 61.44, 15.36, 7.68 and 3.84 MHz; every sample still
> arrives, so an FFT spans the whole rate, but only that 80% is flat
> and calibrated. Sweeping the receiver's own noise floor puts the real
> filter at or beyond the declared edge at decimated rates (flat within
> 0.5 dB across 0.80 x Fs at 15.36 MHz, 0.89 at 7.68 MHz) and just
> inside it at full span (about 1 dB down at the declared edge, 3 dB at
> 0.84 x Fs), which is where Aaronia's 44 MHz data-sheet figure for the
> ECO comes from. Given a span
> it cannot produce, it selects the rate whose alias-free bandwidth
> (0.8 x Fs) is nearest the request: 2.5 MHz yields Fs = 3.84 MHz,
> 1.3 MHz yields 1.92 MHz, and 10 MHz yields 15.36 MHz. A value that is
> itself on the rate ladder round-trips exactly, which is why the field
> behaves like a sample rate in ordinary use. `sampleFrequency` in the
> packet metadata reports the rate in force; `startFrequency` and
> `endFrequency` bound the usable span. Verified on a V6 ECO.

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

#### Save/Reload/Load Mission
```json
{
  "save": true,
  "type": "mission"
}
```

A mission can also be swapped outright, which is how a headless station
changes what it is running. `load` takes an absolute path to an `.rmix`
on the machine running RTSA-Suite, not on the client:
```json
{
  "type": "mission",
  "load": true,
  "file": "C:/Aaronia/Recordings/HTTP streaming/http_test.rmix"
}
```
A mission carrying an HTTP Server block must already be running, or
there is nothing listening to accept the request. Aaronia's remote
control notes (rev 4, May 2026) give this as a `curl` invocation from
an elevated prompt. This crate does not expose it: swapping the mission
out from under a running capture is the caller's decision to make
deliberately, not a side effect of a library call.

**Every `/control` payload needs its `type`.** A body without one is
rejected with `400` and the text `Type argument missing`, so a
hand-built payload that omits it fails loudly rather than silently —
unlike a partial capture request, which is accepted and ignored.

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

**Full PUT form** (config tree). Aaronia's own automation example uses this
shape for every setting it changes:

```json
{
  "request": 11,
  "receiverName": "Block_Spectran_V6B_0",
  "config": {
    "type": "group",
    "items": [
      {
        "type": "group",
        "name": "main",
        "items": [
          { "type": "float", "name": "centerfreq", "value": 2440000000 }
        ]
      }
    ]
  }
}
```

Notes verified against a live V6 ECO:

- **Field names are model-specific.** The V6B example writes `centerfreq`;
  a V6 ECO exposes `centerfreq0` and `centerfreq1` for its two channels.
  Read `GET /remoteconfig` and use the names that device reports.
- **A frequency change needs only the frequency field.** This differs from
  `/control`, where a capture request is ignored unless `frequencyCenter`
  and `frequencySpan` are both present.
- **The `receiverName` key is case-tolerant.** `receivername` was accepted
  identically.
- **Enum-valued settings take either the label string or the index.**
  Aaronia's example writes `decimation` as `"1 / 128"`; writing
  `decimation0` as the integer `3` was accepted and applied identically
  (verified against `/sample`: index 3 gave 7.68 MHz, index 2 gave
  15.36 MHz). Indices are positions in the `values` list that
  `GET /remoteconfig` reports for that item.
- **Groups other than `main` work**, and several can go in one PUT:
  `{"main": {...}, "calibration": {"preamp": 1}}` applied both.
- **An unknown block name is a silent no-op.** A `simpleconfig` PUT
  naming a block that is not in the mission returns HTTP 200 and changes
  nothing. There is no error to catch, so read the value back if it
  matters.
- Other keys the example writes: `run` (bool), `preamp` (`"Auto"`),
  `reflevel` (float), and `filerecord` (bool) plus a filename template on
  a FileWriter block, which is how it starts and stops recording.

**Simplified PUT form**: in addition to the full `{request, config}` shape, the server accepts a shorter per-block write (used by the SDR++ and SDRangel Aaronia plugins), exposed via `HttpEndpointsClient::simple_remote_config`:
```json
{
  "receiverName": "Block_IQDemodulator_0",
  "simpleconfig": {
    "main": { "centerfreq": 100000000.0, "samplerate": 10000000.0, "spanfreq": 10000000.0 }
  }
}
```
`receiverName` must match a block in the running mission;
`HttpEndpointsClient::find_iq_demodulator_block_name` auto-discovers it.
A name that matches nothing still returns 200 (see above).

In the full `{request, config}` form the receiver name is ignored
altogether: the write is routed by `config.name`. Sending
`receivername`, `receiverName` or no name at all applied the same
change. Nothing depends on getting that key right, and nothing warns
when it is wrong.

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

**Subgroups** per health-aware block, per Aaronia's specification:
`info`, `status`, `health`, `settings`, and `components` — the last a
recursive list of sub-blocks when satellites are attached over HTTP,
which is how a remote station's devices show up in a local tree.

**What a V6 ECO actually reports** under `status` and `health`,
alongside the `info` fields below:

| Item | Example | Meaning |
| :--- | :--- | :--- |
| `status/iqsamples` | 61439323 | **Native** IQ rate in Hz, not the delivered one. Measured constant at 61.44 MHz while the same device delivered 15.36, then 7.68, then 61.44 MS/s — it does not follow the decimation setting |
| `status/usbbuffer` | 0.0625 | USB buffer fill, fraction |
| `status/adcrange` | 17.16 | ADC headroom in dB |
| `status/strmtimedist` | 5.96e-06 | Stream time distance in seconds |
| `health/fronttemp` | 64.7 | Frontend temperature, °C |
| `health/fpgatemp` | 52.5 | FPGA temperature, °C |
| `health/boardpower` | 8.975 | Board power draw, W |

`iqsamples` is the one to be careful with: it looks like a sample rate
and is not the one your stream is running at. Read `sampleFrequency`
from packet metadata for that.

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

As with histograms, category samples are a flat number array (one value
per category) — that is what a live server produced.

**A marker stream is not a categories packet.** Aaronia's remote
control notes show the Spectrum block's `Marker` output wired to an
HTTP Server, and the result looks like this:

```json
{
  "payload": "spectra",
  "startFrequency": 0, "endFrequency": 0, "sampleFrequency": 0,
  "sampleSize": 4, "sampleDepth": 1,
  "categories": [
    { "name": "Slow Sine(R)", "startFrequency": 2403000000, "endFrequency": 2405000000 },
    { "name": "Marker",       "startFrequency": 2412852325.3, "endFrequency": 2412852335.3 }
  ],
  "samples": [[-78.6, -57.26, -47.28, -114.25]]
}
```

It carries a `categories` array but declares `payload: "spectra"`, and
spectra samples are a 2D array — one row per frame — so the nesting is
correct for what it says it is, not a contradiction of the flat
categories form above. `sampleSize` is the number of values in a row,
here one per category.

What is worth knowing before parsing one: all three frequency fields
are `0`, so nothing can be derived from them, and the category names
and ranges are the only description of what the numbers mean.

### Antenna Data
Data captured using antennas with location or directional information.

| Field | Description |
| :--- | :--- |
| `name` | Antenna name |
| `latitude` | Antenna latitude |
| `longitude` | Antenna longitude |
| `azimuth` | Azimuth of directional antenna |
| `declination` | Declination of directional antenna |

## What the native SDK samples tell us about this API

Aaronia's [C++ SDK samples](https://github.com/Aaronia-Open-source/RTSA-API-Samples)
drive the same hardware through a different transport, so some of what
they establish applies here and some does not.

### GPS needs enabling, over HTTP too

The `device/gpsmode` and `device/sclksource` keys the `GPSTime` sample
writes are the same keys this config tree exposes, so the prerequisite
is identical. A device ships with `gpsmode` set to `Disabled` and takes
its stream clock from whatever `sclksource` names, which on the system
tested here was `10MHz` rather than GPS. Timestamps are not
GPS-disciplined until both change, and nothing reports an error
meanwhile.

Observed on a V6 ECO: `gpsmode` offers `Disabled`, `Location`, `Time`
and `Location and Time`; `sclksource` offers `Consumer`, `Oscillator`,
`GPS`, `PPS`, `10MHz` and three `... Provider` variants.

### The rate ladder carries over

`decimation0` takes the same labels as the SDK's `main/decimation`,
`"Full"` through `"1 / 512"`, verified by writing one over HTTP. Both
transports drive the same divider. Over HTTP the index works as well as
the label: writing `3` and reading `/sample` back gave 7.68 MHz, and `2`
gave 15.36 MHz, which is 61.44 MHz halved that many times on a V6 ECO.

#### Unresolved: the full V6's top rate

The SDK constrains `spanfreq * 1.5 <= receiverclock`, and a V6 ECO
follows it: its top IQ rate measures 61.44 MHz against the 92.16 MHz
clock the crate assumes for it. `iq_sample_rates_for_clock` takes that
ratio as the rule, so it puts a full V6 on the `245MHz` clock at
163.84 MHz.

Aaronia's own figures do not agree. Their endpoint specification closes
with a note about achieving "the full 250M samples of IQ data", their
block documentation advertises "a real-time bandwidth of up to 245MHz
each" for the V6's two inputs, and both round to the 245.76 MHz that
the `245MHz` clock label denotes — the clock itself, not two thirds of
it.

There is a reading that reconciles them. The clock list runs past
`245MHz` to `492MHz` (491.52 MHz), and 245.76 MHz of span satisfies the
1.5 rule against that clock with room to spare. On that reading 245 MHz
of real-time bandwidth is real but needs the fastest clock, and the
`245MHz` label tops out at 163.84 MHz exactly as the crate computes.

What does not fit either reading is the Remote Config panel Aaronia
published in 2021 and again in 2022: a full V6 with the clock set to
`92MHz` reporting roughly 92.16 MHz of IQ samples per second. That
counter is not the delivered rate — measured on an ECO, `iqsamples`
held at 61.44 MHz while the device was actually delivering 15.36 and
then 7.68 MHz, so it reports the native undecimated rate. Taken at face
value it says a full V6 at a 92 MHz clock has a native rate of
92.16 MHz, which the 1.5 rule forbids.

No full V6 has been available to measure, so this stays open. The
practical guidance is unchanged: the ECO path is verified rung by rung,
and for anything else read the rate the device reports in its stream
metadata rather than trusting a computed ladder.

### Two things that do not carry over

- **Receiver-channel selection.** There is no `device/receiverchannel`
  here. The tree exposes per-channel settings instead — `centerfreq0`
  and `centerfreq1`, `decimation0` and `decimation1`, `rfchsource0` and
  `rfchsource1` — and which channels reach the stream is a property of
  the RTSA mission graph. The SDK's `Rx12` versus `Rx1+Rx2` distinction
  has no equivalent.
- **Stream indices.** The SDK separates IQ from spectra by packet stream
  index. HTTP selects data with the `input=` parameter and reports the
  kind in each packet's `payload` field.

## Authorization & Licensing

### HTTP Streaming vs Remote Configuration

**Basic HTTP Streaming** (No additional license required):
- `/stream` - Real-time data streaming
- `/sample` - Sample retrieval
- `/info` - Device information
- `/healthstatus` - Device health monitoring

**HTTP Server and Client blocks are themselves licensed, and the free
tier is one of each.** Aaronia's staff put it plainly in the HTTP
Server thread: only one HTTP Server instance is included in the free
RTSA-Suite PRO licence and additional instances must be licensed
separately, the same going for the HTTP Client, where only one
connection is free. Stream Merger and Stream Splitter, the blocks that
would otherwise let several streams share one connection, are not in
the free licence either.

This is the licence limit most likely to be met in practice: running
this crate and a second client — a SoapySDR application, say — against
one server at the same time is a second connection. It has nothing to
do with the Remote Config licence discussed below.

**Remote Configuration** (`/remoteconfig`):
- Device parameter configuration.
- Aaronia sells a "Remote Config" license, and this document previously
  stated that `/remoteconfig` writes fail without it. **That is not what
  a live system does.** On RTSA-Suite PRO driving a SPECTRAN V6 ECO whose
  license list contains no Remote Config entry, `PUT /remoteconfig`
  retuned the device repeatedly, in both the config-tree and simplified
  forms, returning HTTP 200 with the change applied.
- What that license actually gates is therefore unconfirmed. It may
  cover a different feature, or a different edition or version, or
  parameters other than the ones tested. Treat any claim that
  `/remoteconfig` writes require it as unverified.
- The system tested did hold "Block: HTTP Server" and "Block: HTTP
  Client" licenses, so the HTTP surface itself is licensed separately
  and may be what actually matters.
- This crate does not depend on the answer: it tunes through `/control`,
  which needs no license.
- Re-confirmed 2026-08-12 against RTSA-Suite PRO and a V6 ECO on the
  same unlicensed system, in both payload forms, for `centerfreq0`,
  `decimation0`, `reflevel0` and `calibration/preamp`.

**License Detection Methods**:

**Important**: reads of `/remoteconfig` are license-free, so a read-only
check cannot prove write capability either way — only a write test can.
It cannot prove the *license* either: writes succeeded on an unlicensed
system (see above), so a successful probe means "this server accepts
writes", not "this server is licensed".

The client therefore exposes two methods:

- `detect_remote_config_license()` — **read-only**. Never touches device state. Classifies 401/403 responses; on read success it returns `Unknown` (write capability unproven).
- `probe_remote_config_write_license()` — **active probe**. Performs a read-modify-restore cycle on `reflevel` (+1 dB, restored best-effort) to positively verify write capability. Use only when you genuinely need proof of `/remoteconfig` write access. `AaroniaSource::probe_remote_config_license()` delegates to it for HTTP sources, and reports `Active` for the file and native-SDK backends, which do not need the licence. Frequency hopping does **not** need it — retuning goes through the license-free `/control` endpoint (see the capture-control note above; the silent-ignore behavior once attributed to licensing was traced to partial capture payloads).

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
| 2.4 | 2026-08-12 | Added Aaronia support's full `/control` settings list (`deviceconnect`, `camera`, per-type fields, `receiverUUID`/`receiverName` scoping), which supersedes the specification's claim that commands cannot be addressed to a block; documented that an unrecognised `format=` silently serves the RTSA file format and that `raw16` aliases `int16`, both verified live |
| 2.3 | 2026-08-12 | Folded in Aaronia's endpoint specification (rev 11) and the block forum threads: `/control` broadcasts to every block and is PUT-only, the server drops data past an 8 MB outbound buffer, `/healthstatus` subgroups and the fields a V6 ECO reports, and the one-server/one-client free-licence limit. Measured that `status/iqsamples` is the native rate, not the delivered one. Corrected the marker-stream entry: it declares `payload: "spectra"`, so its nested samples are the spectra form and not a counter-example to flat categories |
| 2.2 | 2026-08-12 | Verified Aaronia's V6 remote control notes (rev 4) against hardware: enum writes by index, multi-group and non-`main` `simpleconfig` PUTs, the silent no-op on an unknown block name, the ignored receiver name in the config-tree form; documented mission loading and the `type` requirement on `/control`, the absence of a status endpoint, and the unresolved conflict over what "Full" means on a full V6; resolved a contradiction over what the Remote Config licence gates |
| 2.1 | 2026-08-06 | Live-hardware corrections folded in (two-byte separator, spectra frame counting, `scale` inversion); documented `/samples`, the `/sample` TX push, and the `simpleconfig` PUT form; corrected `limit` semantics, field types, and flat histogram/categories sample arrays; removed decorative icons |
---

## Sources and Attribution

This specification is a compiled, community-maintained document. It is **not**
published or endorsed by Aaronia AG, and it may lag or diverge from the
vendor's own materials. For authoritative, vendor-published references,
consult:

- **RTSA-Suite PRO HTTP streaming** (Aaronia V6 forum) — [v6-forum.aaronia.de/forum/topic/rtsa-suite-pro-http-streaming](https://v6-forum.aaronia.de/forum/topic/rtsa-suite-pro-http-streaming/)
- Aaronia RTSA-Suite PRO product documentation — <https://rtsa-manual.aaronia.com/>
- **Aaronia's own automation example** — [Aaronia-Open-source/python_RTSA_HTTP_API_Sequence_Example](https://github.com/Aaronia-Open-source/python_RTSA_HTTP_API_Sequence_Example). A Python script that runs a measurement sequence and records to disk, driving the device entirely through `PUT /remoteconfig`. It is the source of the config-tree write example below.

The content here is derived from the above plus empirical analysis of live
RTSA-Suite PRO streams. "Aaronia", "RTSA", and "Spectran" are the property of
Aaronia AG.

---

*This specification covers the complete Aaronia RTSA HTTP streaming protocol for real-time RF data access. For file format specifications, see FILESPEC.md.*