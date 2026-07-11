# sdr-aaronia-rs

[![Crates.io](https://img.shields.io/crates/v/sdr-aaronia-rs.svg)](https://crates.io/crates/sdr-aaronia-rs)
[![Docs.rs](https://docs.rs/sdr-aaronia-rs/badge.svg)](https://docs.rs/sdr-aaronia-rs)
[![CI](https://github.com/isaacbentley/sdr-aaronia-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/isaacbentley/sdr-aaronia-rs/actions/workflows/ci.yml)
[![License: GPL-3.0-or-later](https://img.shields.io/github/license/isaacbentley/sdr-aaronia-rs.svg)](https://choosealicense.com/licenses/gpl-3.0/)

A comprehensive Rust library for interfacing with Aaronia Spectran SDR devices. 

*Disclaimer: This project is not affiliated with Aaronia AG. Aaronia, SPECTRAN, and RTSA-Suite PRO are trademarks of Aaronia AG.*

`sdr-aaronia-rs` provides a unified API for interacting with Aaronia hardware, abstracting away the underlying transport layers. It supports native SDK connections, bidirectional HTTP streaming (RX/TX), and RTSA file sources through a single, consistent interface with deterministic source selection.

## Overview

Interfacing with SDR hardware typically requires choosing between proprietary native SDKs, HTTP streaming protocols, or file-based playback. Each approach demands a different API and configuration lifecycle.

`sdr-aaronia-rs` solves this by offering a unified `AaroniaSource` that automatically selects the optimal transport based on your configuration:
1. **File Path** → Utilizes the buffered RTSA file source.
2. **HTTP URL** → Utilizes the HTTP streaming source.
3. **No Target** → Defaults to the native SDK, falling back to `localhost:54664`.
4. **Explicit Force** → Locks the source to a specific backend.

## Key Features

- **Native SDK Integration:** Direct hardware access via Aaronia RTSA-Suite PRO with zero-copy sample processing, real-time IQ data, and automatic platform library detection. Enforces hardware constraints (e.g., `span * 1.5 ≤ receiverclock`) prior to streaming.
- **Advanced HTTP Streaming (RX & TX):** Supports JSON, Int16, Float16, and Float32 streaming formats over chunked HTTP connections. Implements the complete RTSA HTTP specification with Basic Auth and token-based enterprise authentication, including a dynamic HTTP TX sink for transmitting IQ data.
- **RTSA File Processing:** Full RTSA specification implementation for reading capture files via buffered I/O, including metadata extraction and multi-stream support.
- **Device Management:** Real-time control of streaming parameters, device health monitoring, input stream enumeration, and hierarchical configuration.
- **FutureSDR Integration:** Provides seamless integration with FutureSDR flowgraphs via native blocks.

*Note: Device configuration via the HTTP endpoint requires a separate Aaronia "Remote Config" license.*

## Installation

Add the following to your `Cargo.toml`:

```toml
[dependencies]
# By default, includes HTTP, File, native sdr-source trait, and C FFI backend support
sdr-aaronia-rs = "0.2"
tokio = { version = "1.43", features = ["rt-multi-thread", "macros"] }

# To enable additional backends, opt into their features (e.g. native-sdk, futuresdr)
# sdr-aaronia-rs = { version = "0.2", features = ["native-sdk", "futuresdr"] }
```

## 60-Second Quickstart

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
    
    // Read IQ samples seamlessly!
    let mut buffer = Vec::with_capacity(1024);
    let n = source.read_samples(&mut buffer, 1024).await?;
    println!("Received {} IQ samples", n);

    Ok(())
}
```

## Usage Examples

We provide ready-to-run examples in the `examples/` directory demonstrating each subsystem:
* **[http_iq_quickstart.rs](examples/http_iq_quickstart.rs)**: Connect to an RTSA HTTP server and stream live IQ data natively.
* **[noaa_scanner.rs](examples/noaa_scanner.rs)**: Scan NOAA weather channels and demodulate FM audio in real-time using `FutureSDR`.
* **[native_sdk_basic.rs](examples/native_sdk_basic.rs)**: Access hardware directly with zero-copy C++ Native SDK integration.
* **[native_sdk_transmit.rs](examples/native_sdk_transmit.rs)**: Stream IQ bursts (e.g. LoRa chirps) over the Native SDK to standard Spectran V6 devices.
* **[channel_hopping.rs](examples/channel_hopping.rs)**: Perform automatic frequency hopping using the native `sdr-source` feature.
* **[read_rtsa_file.rs](examples/read_rtsa_file.rs)**: Open a local RTSA capture, parse the metadata headers, and read samples efficiently.
* **[dump_metadata.rs](examples/dump_metadata.rs)**: Inspect the DSFH metadata tree inside `.rtsa` captures for debugging.
* **[device_control.rs](examples/device_control.rs)**: Perform device health checks, list available inputs, and safely interact with Aaronia HTTP endpoints.

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

`AaroniaSourceBuilder` is the high-level unified builder. It does *not* take a URL — connection details are auto-detected, and IQ-mode parameters are expressed cleanly.

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

    // API is identical regardless of which backend was selected!
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

For existing [FutureSDR](https://github.com/FutureSDR/FutureSDR) users, the low-level block API seamlessly integrates high-throughput streams (both RX and TX) into a flowgraph:

```rust,ignore,no_run
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

The low-level `HttpSourceBuilder` offers advanced properties (e.g., `buffer_size`, `timeout_ms`, `rate_reduction`) and authentication settings:

```rust,ignore,no_run
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

## Environment Variables

| Variable | Effect |
|---|---|
| `AARONIA_SDK_PATH` | Overrides the RTSA-Suite installation directory used for SDK / `RTSAFileTool` detection. Works on every platform (it is the only way to point at an install on macOS, where there is no default path). |
| `AARONIA_USER_AGENT` | Overrides the HTTP `User-Agent` string sent by every outbound request (default: `sdr-aaronia-rs/<version>`). |

## Remote Config License Detection

Read access to `/remoteconfig` works without a license, so a read-only check cannot prove write capability. `HttpEndpointsClient` exposes both options:

- `detect_remote_config_license()` — read-only, never touches device state; returns `Unknown` when reads succeed.
- `probe_remote_config_write_license()` — **actively verifies** writes by temporarily adjusting `reflevel` by +1 dB (restored best-effort). `AaroniaSource::probe_remote_config_license()` uses this before frequency hopping, because an unlicensed retune returns HTTP 200 and is silently ignored server-side.

## Feature Flags

`sdr-aaronia-rs` uses a modular feature matrix to minimize its dependency footprint:

| Feature | Description | Default |
|---|---|---|
| `http` | HTTP streaming via `reqwest` and `tokio`. | **Yes** |
| `file` | Buffered RTSA file parsing. | **Yes** |
| `native-sdk` | Links the proprietary Aaronia C++ SDK. | No |
| `futuresdr` | Integrates `HttpSource` as a FutureSDR source block. | No |
| `sdr-source` | Integrates `AaroniaSdrSource` implementing the native `SdrSource` traits. | **Yes** |
| `ffi` | Builds the C-API export layer. | **Yes** |

## MSRV & Semver Policy

- **MSRV:** This crate does not maintain an explicit Minimum Supported Rust Version (MSRV) policy and tracks the latest `stable` compiler.
- **Semver:** This crate follows semantic versioning. While in `0.x.y`, breaking API changes will result in a minor version bump (e.g. `0.1.x` to `0.2.0`). MSRV bumps will also occur on minor version releases.

## Testing & Contributing

We maintain a layered test pyramid consisting of unit tests, integration tests against LFS captures, and property tests enforcing specification invariants. 

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for detailed instructions on running the test suite, generating coverage reports, and formatting your code before submitting a Pull Request.

## Documentation

- [Architecture & Design](DESIGN.md) — Internal architecture and execution flow.

## License

This project is licensed under the GNU General Public License v3.0 or later (GPL-3.0-or-later) - see the LICENSE file for details.
