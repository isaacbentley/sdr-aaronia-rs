# sdr-aaronia-rs

[![Crates.io](https://img.shields.io/crates/v/sdr-aaronia-rs.svg)](https://crates.io/crates/sdr-aaronia-rs)
[![Docs.rs](https://docs.rs/sdr-aaronia-rs/badge.svg)](https://docs.rs/sdr-aaronia-rs)
[![CI](https://github.com/isaacbentley/sdr-aaronia-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/isaacbentley/sdr-aaronia-rs/actions/workflows/ci.yml)
[![License: GPL-3.0-or-later](https://img.shields.io/github/license/isaacbentley/sdr-aaronia-rs.svg)](https://choosealicense.com/licenses/gpl-3.0/)

A Rust library for interfacing with Aaronia Spectran SDR devices.

*Disclaimer: This project is not affiliated with Aaronia AG. Aaronia, SPECTRAN, and RTSA-Suite PRO are trademarks of Aaronia AG.*

`sdr-aaronia-rs` provides a unified API for interacting with Aaronia hardware, abstracting away the underlying transport layers. It supports native SDK connections, bidirectional HTTP streaming (RX/TX), and RTSA file sources through a single, consistent interface with deterministic source selection.

In addition to the Rust crate, this project provides **Python bindings** (via PyO3) with single-copy NumPy/Arrow reads, a **C++ SoapySDR plugin**, and a **Rust-native seify backend**.

## Overview

Interfacing with SDR hardware typically requires choosing between proprietary native SDKs, HTTP streaming protocols, or file-based playback. Each approach demands a different API and configuration lifecycle.

`sdr-aaronia-rs` solves this by offering a unified `AaroniaSource` that automatically selects the optimal transport based on your configuration:
1. **File Path** → Utilizes the buffered RTSA file source.
2. **HTTP URL** → Utilizes the HTTP streaming source.
3. **No Target** → Defaults to the native SDK, falling back to `localhost:54664`.
4. **Explicit Force** → Locks the source to a specific backend.

## Key Features

- **Native SDK Integration:** Direct hardware access via Aaronia RTSA-Suite PRO with zero-copy sample processing, real-time IQ data, and automatic platform library detection. Enforces hardware constraints (e.g., `span * 1.5 ≤ receiverclock`) prior to streaming. Windows and Linux only (`native-sdk` feature); Aaronia does not ship a macOS SDK.
- **HTTP Streaming (RX & TX):** Supports JSON, Int16, Float16, and Float32 streaming formats over chunked HTTP connections, with Basic Auth and token-based authentication. An HTTP TX sink for transmitting IQ data is available under the `futuresdr` feature.
- **RTSA File Processing:** Reads RTSA capture files via buffered I/O, including metadata extraction and multi-stream support.
- **Device Management:** Real-time control of streaming parameters, device health monitoring, input stream enumeration, and hierarchical configuration.
- **FutureSDR Integration:** Optional `HttpSource` and `HttpSink` flowgraph blocks under the `futuresdr` feature.
- **Python Data-Science Native:** PyO3 bindings with single-copy reads into NumPy arrays and Apache Arrow buffers (one copy out of the Rust receive buffer per read).
- **SDR Ecosystem Plugins:** a C++ `SoapySDR` plugin and a Rust-native `seify` backend.

*Note: writes to the HTTP `/remoteconfig` endpoint require a separate Aaronia "Remote Config" license; capture control via `/control` (including retuning) does not.*

## Installation

Add the following to your `Cargo.toml`:

```toml
[dependencies]
# By default, includes HTTP, File, native sdr-source trait, and C FFI backend support
sdr-aaronia-rs = "0.5"
tokio = { version = "1.43", features = ["rt-multi-thread", "macros"] }

# To enable additional backends, opt into their features (e.g. native-sdk, futuresdr)
# sdr-aaronia-rs = { version = "0.5", features = ["native-sdk", "futuresdr"] }
```

## Quickstart

Specify your RF parameters and `sdr-aaronia-rs` will auto-detect the optimal backend (Native SDK, HTTP Streaming, or RTSA File):

```rust,no_run
use sdr_aaronia_rs::{AaroniaSource, AaroniaConfig};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // Let the library auto-detect the best backend
    let config = AaroniaConfig::default()
        .center_frequency(446.0e6)     // 446 MHz
        .span_frequency(10.0e6)        // 10 MHz span
        .reference_level(-30.0);       // -30 dBm

    let mut source = AaroniaSource::new(config).await?;

    // Read IQ samples through the unified interface
    let mut buffer = Vec::with_capacity(1024);
    let n = source.read_samples(&mut buffer, 1024).await?;
    println!("Received {} IQ samples", n);

    Ok(())
}
```

## Usage Examples

The `examples/` directory contains runnable examples for each subsystem:

- **[http_iq_quickstart.rs](examples/http_iq_quickstart.rs)**: Connect to an RTSA HTTP server and stream live IQ data natively.
- **[noaa_scanner.rs](examples/noaa_scanner.rs)**: Scan NOAA weather channels and demodulate FM audio in real-time using `FutureSDR`.
- **[native_sdk_basic.rs](examples/native_sdk_basic.rs)**: Access hardware directly through the vendor's native SDK (Windows/Linux).
- **[native_sdk_transmit.rs](examples/native_sdk_transmit.rs)**: Stream IQ bursts (e.g. LoRa chirps) over the Native SDK to standard Spectran V6 devices.
- **[channel_hopping.rs](examples/channel_hopping.rs)**: Perform automatic frequency hopping using the native `sdr-source` feature.
- **[read_rtsa_file.rs](examples/read_rtsa_file.rs)**: Open a local RTSA capture, parse the metadata headers, and read samples efficiently.
- **[dump_metadata.rs](examples/dump_metadata.rs)**: Inspect the DSFH metadata tree inside `.rtsa` captures for debugging.
- **[device_control.rs](examples/device_control.rs)**: Perform device health checks, list available inputs, and safely interact with Aaronia HTTP endpoints.

### Unified API (Auto-Detection)

The recommended approach is to specify the RF parameters and let the library handle backend selection.

```rust,no_run
use sdr_aaronia_rs::{AaroniaSource, AaroniaConfig};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // Specify RF parameters; the library auto-detects the best backend
    let config = AaroniaConfig::default()
        .center_frequency(446.0e6)     // 446 MHz UHF amateur
        .span_frequency(10.0e6)        // 10 MHz span
        .reference_level(-30.0);       // -30 dBm

    let mut source = AaroniaSource::new(config).await?;
    println!("Selected Source: {:?}", source.get_source_info());

    // Read IQ samples using the unified interface
    let mut samples = Vec::with_capacity(1024);
    let n = source.read_samples(&mut samples, 1024).await?;
    println!("Received {} IQ samples", n);

    Ok(())
}
```

### Builder Pattern with Auto-Detection

`AaroniaSourceBuilder` is the high-level unified builder. By default the backend is auto-detected, but it can be pinned explicitly with `http_source(url)`, `file_source(path)`, or `force_source_type(...)`; additional knobs include `device_serial(...)`, `stream_format(...)`, `stream_scale(...)`, and `receiver_channel(...)` (native-SDK RX selection, incl. dual-channel `Rx1And2`).

```rust,no_run
use sdr_aaronia_rs::AaroniaSourceBuilder;
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let mut builder = AaroniaSourceBuilder::new();
    builder
        .center_frequency(2.44e9)     // 2.4 GHz ISM band
        .span_frequency(20.0e6)       // 20 MHz span
        .reference_level(-25.0);      // -25 dBm

    let mut source = builder.build().await?;

    // The API is identical regardless of which backend was selected
    let mut samples = Vec::with_capacity(1024);
    source.read_samples(&mut samples, 1024).await?;

    Ok(())
}
```

### Explicit Source Selection

You can force a specific backend if auto-detection is not desired:

```rust,no_run
use sdr_aaronia_rs::{AaroniaSource, AaroniaConfig};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // Force Native SDK
    let sdk_config = AaroniaConfig::default()
        .center_frequency(2.44e9)
        .span_frequency(20.0e6)
        .reference_level(-20.0)
        .force_native_sdk();
    let _sdk_source = AaroniaSource::new(sdk_config).await?;

    // Force HTTP Streaming
    let http_config = AaroniaConfig::from_http("http://192.168.1.100");
    let _http_source = AaroniaSource::new(http_config).await?;

    // Force RTSA File Source
    let file_config = AaroniaConfig::from_file("capture.rtsa");
    let _file_source = AaroniaSource::new(file_config).await?;

    Ok(())
}
```

### Bandwidth vs. Precision Tradeoffs (HTTP Streaming)

When using the HTTP backend over a network link, the wire format heavily impacts bandwidth. `sdr-aaronia-rs` defaults to lossless Float32 for maximum precision, but you can opt into a low-bandwidth integer mode if network throughput is a bottleneck.

```rust,no_run
use sdr_aaronia_rs::AaroniaConfig;

// Default (Float32): 8 bytes/sample on the wire. Lossless, zero-copy decode.
// At 92 MSPS, requires ~740 MB/s network throughput (best for localhost).
let _high_fidelity = AaroniaConfig::default()
    .center_frequency(2.4e9);

// Low Bandwidth (Int16): 4 bytes/sample. Halves network traffic.
// Requires setting both the format and the encode scale factor.
// At 92 MSPS, requires ~370 MB/s.
let _low_bandwidth = AaroniaConfig::default()
    .center_frequency(2.4e9)
    .low_bandwidth_mode(); // Sets StreamFormat::Int16 and scale=32767.0
```

### Custom Configuration Profiles

Build your own configuration for specific bands:

```rust,no_run
use sdr_aaronia_rs::AaroniaConfig;

// UHF amateur band
let _config = AaroniaConfig::default()
    .center_frequency(446.0e6)    // 446 MHz
    .span_frequency(10.0e6)       // 10 MHz span
    .reference_level(-30.0);      // -30 dBm

// 2m amateur band
let _config = AaroniaConfig::default()
    .center_frequency(146.52e6)   // 2m amateur
    .span_frequency(25e3)         // 25 kHz
    .reference_level(-30.0);      // -30 dBm
```

### Device Control & Monitoring

Use the `HttpEndpointsClient` to manage the physical device state, retrieve health telemetry, and manage stream inputs.

```rust,no_run
use sdr_aaronia_rs::{HttpEndpointsClient, AuthMethod};
use sdr_aaronia_rs::http_endpoints::{CaptureControl, ControlType};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let client = HttpEndpointsClient::new(
        "http://127.0.0.1:54664".to_string(),
        AuthMethod::None
    )?;

    // Retrieve device information
    let info = client.get_info().await?;
    println!("Connected to: {} ({})", info.title, info.name);

    // Start/stop streaming
    client.control_streaming(true).await?;

    // Apply capture configuration
    let config = CaptureControl {
        frequency_center: Some(162.4e6),
        frequency_span: Some(25e3),
        reference_level: Some(-20.0),
        control_type: ControlType::Capture,
        ..Default::default()
    };
    client.configure_capture(config).await?;

    Ok(())
}
```

### FutureSDR Integration (Advanced)

For existing [FutureSDR](https://github.com/FutureSDR/FutureSDR) users, the low-level block API integrates high-throughput streams (both RX and TX) into a flowgraph. `HttpSourceBuilder`, `HttpSinkBuilder`, and the corresponding blocks require the `futuresdr` feature:

```rust,ignore
use sdr_aaronia_rs::{HttpSourceBuilder, HttpSinkBuilder};
use futuresdr::runtime::Flowgraph;
use anyhow::Result;

fn main() -> Result<()> {
    // Low-level FutureSDR blocks
    let source = HttpSourceBuilder::new("http://127.0.0.1:54664")
        .frequency(146.52e6)
        .sample_rate(25e3)
        .build()?;

    let sink = HttpSinkBuilder::new("http://127.0.0.1:54664")
        .frequency(433.0e6)
        .sample_rate(1e6)
        .build()?;

    // Use in FutureSDR flowgraph
    let mut fg = Flowgraph::new();
    let _src = fg.add_block(source);
    let _sink = fg.add_block(sink);
    // ... connect to other blocks

    Ok(())
}
```

### Advanced Streaming with Authentication

The low-level `HttpSourceBuilder` (also part of the `futuresdr` feature) offers advanced properties (e.g., `buffer_size`, `timeout_ms`, `rate_reduction`) and authentication settings:

```rust,ignore
use sdr_aaronia_rs::{AuthMethod, HttpSourceBuilder, StreamFormat};
use anyhow::Result;

fn main() -> Result<()> {
    let _advanced_source = HttpSourceBuilder::new("http://127.0.0.1:54664")
        .frequency(446.125e6)           // UHF band
        .sample_rate(12.5e3)            // Narrow bandwidth
        .format(StreamFormat::Int16)    // High-performance format
        .auth(AuthMethod::Basic {
            username: "admin".to_string(),
            password: "secure_pass".to_string(),
        })
        .input("main")                  // Specific input stream
        .rate_reduction(4)              // Bandwidth optimization
        .buffer_size(16384)             // Large buffer
        .timeout_ms(1000)               // Fast timeout
        .build()?;
    Ok(())
}
```

### Low-Level Asynchronous Stream Reading

```rust,no_run
use sdr_aaronia_rs::http_endpoints::{HttpEndpointsClient, AuthMethod, StreamParamsBuilder};
use sdr_aaronia_rs::http_streaming::StreamFormat;
use futures::stream::StreamExt;
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let client = HttpEndpointsClient::new(
        "http://127.0.0.1:54664".to_string(),
        AuthMethod::None
    )?;

    let stream_params = StreamParamsBuilder::new()
        .format(StreamFormat::Float32)
        .input("main".to_string())
        .build();

    let mut stream = client.start_stream(stream_params).await?;

    while let Some(packet_result) = stream.next().await {
        match packet_result {
            Ok(packet) => println!("Received packet with {} samples", packet.samples.len()),
            Err(e) => eprintln!("Error receiving packet: {}", e),
        }
    }

    Ok(())
}
```

### Additional Device Management

**Recording Control**
```rust,no_run
use sdr_aaronia_rs::{HttpEndpointsClient, AuthMethod};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let client = HttpEndpointsClient::new("http://127.0.0.1:54664".to_string(), AuthMethod::None)?;
    client.control_recording(true, Some("my_recording".to_string())).await?;
    client.control_recording(false, None).await?;
    Ok(())
}
```

**Input Management**
```rust,no_run
use sdr_aaronia_rs::{HttpEndpointsClient, AuthMethod};
use sdr_aaronia_rs::http_endpoints::InputProcessingType;
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let client = HttpEndpointsClient::new("http://127.0.0.1:54664".to_string(), AuthMethod::None)?;
    let _inputs = client.get_inputs().await?;
    let _new_input = client.create_input("main", InputProcessingType::Average).await?;
    Ok(())
}
```

**Token Authentication Flow**
```rust,no_run
use sdr_aaronia_rs::{HttpEndpointsClient, AuthMethod};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let client = HttpEndpointsClient::new("http://127.0.0.1:54664".to_string(), AuthMethod::None)?;
    let user = client.get_user().await?;
    let _auth = AuthMethod::Token { token: user.token };
    Ok(())
}
```

## Python Bindings (NumPy & Apache Arrow)

The `python-aaronia` package provides Python bindings to the Rust engine
using PyO3. Reads land in NumPy or PyArrow with exactly one copy out of
the Rust receive buffer; blocking calls release the GIL. Wheels are
abi3 (CPython ≥ 3.9); the PyPI distribution is `python-aaronia`, and
the importable module is `aaronia`.

### Installation
From a published release:
```bash
pip install python-aaronia
```
Or build locally with `maturin`:
```bash
cd python-aaronia
maturin develop --release
```

### Python Streaming Example
```python
from aaronia import AaroniaConfig, AaroniaSource

config = AaroniaConfig()
config.http_base_url = "http://localhost:54664"  # RTSA-Suite HTTP server
config.format = "F32"
config.center_freq = 2400e6
config.sample_rate = 20e6

# Connect to the stream
source = AaroniaSource()
source.start_streaming(config)

# 1. NumPy read (1D complex64; blocks with the GIL released)
np_samples = source.read_samples_numpy(1024)
print(f"NumPy shape: {np_samples.shape}")

# 2. Apache Arrow Dataframe Integration
arrow_samples = source.read_samples_arrow(1024)
print(f"Arrow records: {len(arrow_samples)}")

# Health Metrics
drops = source.cumulative_drops()
overrun = source.take_overrun()
print(f"Stream Drops: {drops}, Buffer Overrun: {overrun}")

source.stop_streaming()
```

## C++ SDR Plugins (SoapySDR & Seify)

The `sdr-aaronia-rs` workspace also acts as the source of truth for standard SDR ecosystem drivers:

- **SoapySDR Plugin** (`soapy-aaronia/`, C++): CF32 and CS16 stream
  formats (CS16 is a client-side conversion; for genuine low-bandwidth
  *network* streaming pass the `format=I16` device arg, which switches
  the HTTP wire format), retune-safe streaming, honoured timeouts,
  timestamps and dropped-block telemetry. TX exists behind the
  native SDK on Windows/Linux and is hardware-unverified.
- **Seify backend** (Rust-native, `seify` feature): construct via
  `AaroniaSeifyDevice::from_args`; not part of seify's built-in
  enumeration registry.

For deep-dive setup instructions and documentation on the C++ side and Bandwidth Optimization tricks, please see the dedicated [PLUGINS.md](PLUGINS.md) document.

## Environment Variables

| Variable | Effect |
|---|---|
| `AARONIA_SDK_PATH` | Overrides the RTSA-Suite installation directory used for SDK / `RTSAFileTool` detection. Works on every platform. On macOS — which has no default install path and no native SDK build — it only affects `RTSAFileTool` and XML-config detection. |
| `AARONIA_USER_AGENT` | Overrides the HTTP `User-Agent` string sent by every outbound request (default: `sdr-aaronia-rs/<version>`). |

## Remote Config License Detection

Read access to `/remoteconfig` works without a license, so a read-only check cannot prove write capability. `HttpEndpointsClient` exposes both options:

- `detect_remote_config_license()` — read-only, never touches device state; returns `Unknown` when reads succeed.
- `probe_remote_config_write_license()` — **actively verifies** writes by temporarily adjusting `reflevel` by +1 dB (restored best-effort). `AaroniaSource::probe_remote_config_license()` delegates to it for HTTP sources.

The hopping orchestrator itself retunes through the license-free `/control` endpoint and does not gate on this probe; the license only affects `/remoteconfig` writes.

## Feature Flags

Functionality is grouped behind Cargo features so unused dependencies stay out of your build:

| Feature | Description | Default |
|---|---|---|
| `http` | HTTP streaming via `reqwest` and `tokio`. | **Yes** |
| `file` | Buffered RTSA file parsing. | **Yes** |
| `native-sdk` | Links the proprietary Aaronia C++ SDK. Windows/Linux only. | No |
| `futuresdr` | Enables the FutureSDR block API: `HttpSource`, `HttpSink`, and their builders. Implies `http`. | No |
| `sdr-source` | Integrates `AaroniaSdrSource` implementing the native `SdrSource` traits. | **Yes** |
| `ffi` | Builds the C-API export layer. | **Yes** |

## MSRV & Semver Policy

- **MSRV:** This crate does not maintain an explicit Minimum Supported Rust Version (MSRV) policy and tracks the latest `stable` compiler.
- **Semver:** This crate follows semantic versioning. While in `0.x.y`, breaking API changes will result in a minor version bump (e.g. `0.1.x` to `0.2.0`). MSRV bumps will also occur on minor version releases.

## Testing & Contributing

The test suite consists of unit tests, integration tests against LFS captures, and property tests enforcing specification invariants.

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for detailed instructions on running the test suite, generating coverage reports, and formatting your code before submitting a Pull Request.

## Documentation

- [Architecture & Design](DESIGN.md) — Internal architecture and execution flow.
- [RTSA File Format Specification](docs/FILESPEC.md) — On-disk `.rtsa` capture-file format and how this crate parses it.
- [HTTP API Specification](docs/HTTPSPEC.md) — The RTSA HTTP streaming and control API.
- [Native SDK Specification](docs/SDKSPEC.md) — The Aaronia RTSA-Suite PRO SDK surface and the Rust binding notes.
- [Changelog](CHANGELOG.md) — Release history.

## License

This project is licensed under the GNU General Public License v3.0 or later (GPL-3.0-or-later) - see the LICENSE file for details.
