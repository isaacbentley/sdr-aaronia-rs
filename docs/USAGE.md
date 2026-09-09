# Usage

Worked examples for each part of the API. Every Rust snippet here is
compiled as a doctest, so it stays valid as the API changes.

For first-time setup of the RTSA-Suite HTTP Server block, see
[QUICKSTART.md](QUICKSTART.md). For runnable programs, see
[Runnable examples](#runnable-examples) below.

## Unified API with auto-detection

Specify the RF parameters and let the library select the backend.

```rust,no_run
use sdr_aaronia_rs::{AaroniaSource, AaroniaConfig};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // Specify RF parameters; the library auto-detects the best backend
    let config = AaroniaConfig::default()
        .center_frequency_hz(446.0e6)  // 446 MHz UHF amateur
        .sample_rate_hz(10.0e6)        // 10 MS/s (Fs)
        .reference_level_dbm(-30.0);   // -30 dBm

    let mut source = AaroniaSource::new(config).await?;
    println!("Selected Source: {:?}", source.get_source_info());

    // Read IQ samples using the unified interface
    let mut samples = Vec::with_capacity(1024);
    let n = source.read_samples(&mut samples, 1024).await?;
    println!("Received {} IQ samples", n);

    Ok(())
}
```

## Builder pattern

`AaroniaSourceBuilder` is the high-level unified builder. By default the backend is auto-detected, but it can be pinned explicitly with `http_source(url)`, `file_source(path)`, or `force_source_type(...)`; additional knobs include `device_serial(...)`, `stream_format(...)`, `stream_scale(...)`, and `receiver_channel(...)` (native-SDK RX selection, incl. dual-channel `Rx1And2`).

```rust,no_run
use sdr_aaronia_rs::AaroniaSourceBuilder;
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let mut builder = AaroniaSourceBuilder::new();
    builder
        .center_frequency_hz(2.44e9)  // 2.4 GHz ISM band
        .sample_rate_hz(20.0e6)       // 20 MS/s (Fs)
        .reference_level_dbm(-25.0);  // -25 dBm

    let mut source = builder.build().await?;

    // The API is identical regardless of which backend was selected
    let mut samples = Vec::with_capacity(1024);
    source.read_samples(&mut samples, 1024).await?;

    Ok(())
}
```

## Explicit source selection

You can force a specific backend if auto-detection is not desired:

```rust,no_run
use sdr_aaronia_rs::{AaroniaSource, AaroniaConfig};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // Force Native SDK
    let sdk_config = AaroniaConfig::default()
        .center_frequency_hz(2.44e9)
        .sample_rate_hz(20.0e6)
        .reference_level_dbm(-20.0)
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

## Wire format and network bandwidth

When using the HTTP backend over a network link, the wire format heavily impacts bandwidth. `sdr-aaronia-rs` defaults to lossless Float32 for maximum precision, but you can opt into a low-bandwidth integer mode if network throughput is a bottleneck.

```rust,no_run
use sdr_aaronia_rs::AaroniaConfig;

// Default (Float32): 8 bytes/sample on the wire. Lossless, zero-copy decode.
// At the 61.44 MS/s top rate that is ~490 MB/s (best for localhost).
let _high_fidelity = AaroniaConfig::default()
    .center_frequency_hz(2.4e9);

// Low Bandwidth (Int16): 4 bytes/sample. Halves network traffic.
// Requires setting both the format and the encode scale factor.
// At 61.44 MS/s, ~246 MB/s.
let _low_bandwidth = AaroniaConfig::default()
    .center_frequency_hz(2.4e9)
    .low_bandwidth_mode(); // Sets StreamFormat::Int16 and scale=32767.0
```

### Will the link carry it?

The span picks a sample rate off the device's decimation ladder, and at
4 bytes a sample (Int16/Float16) that rate is a byte rate the whole path
has to sustain. Measured on a SPECTRAN V6 ECO over gigabit: `--span 10M`
(15.36 MS/s, 61.4 MB/s) came back contiguous, while `--span 20M`
(30.72 MS/s, 122.9 MB/s) lost 1.84 s of a 35 s capture — dropped by the
*server*, upstream of the client, and each gap is a discontinuity that
breaks digital symbol timing.

`link_budget` answers the question before the capture rather than after:

```rust
use sdr_aaronia_rs::link_budget::{max_sustainable_bandwidth, required_byte_rate};

// What a span costs, via the ladder.
let rate = sdr_aaronia_rs::iq_sample_rate_for_bandwidth(10e6);       // 15.36 MS/s
assert_eq!(required_byte_rate(rate), Some(61_440_000.0));            // 61.4 MB/s

// What a measured path affords, as a span you can pass to --span.
// `None` would mean no rung fits (or no budget can be computed at all,
// e.g. for the JSON format) — never a 0.0 a comparison could wave through.
assert_eq!(max_sustainable_bandwidth(75_000_000.0), Some(12_288_000.0));  // 12.288 MHz
```

To measure the path rather than assume it, stream from the server and
count bytes off the socket. The probe discards an initial settle window
(`LINK_PROBE_SETTLE`, 500 ms) first, because the RTSA server hands over
its pre-connect backlog faster than real time — count that and the
answer comes out *above* the true link rate, which is the one error that
makes a probe worse than none:

```rust,no_run
# async fn probe() -> sdr_aaronia_rs::Result<()> {
use std::time::Duration;
use sdr_aaronia_rs::link_budget::{DEFAULT_LINK_FORMAT, measure_link_throughput};

let m = measure_link_throughput("http://localhost:54664", Duration::from_secs(3)).await?;
println!("{m}");                                   // rate, window, settle discarded
println!(
    "widest span: {:?} Hz",
    m.max_sustainable_bandwidth(DEFAULT_LINK_FORMAT)
);
# Ok(())
# }
```

`measure_link_throughput_with` additionally takes the capture's
`StreamParams` — probe with the same format, input and rate reduction
the capture will use, or the probe measures a different stream — and the
settle window, for servers whose connect backlog outlasts the default.

It measures what the path *delivered*, which is a floor on what it can
deliver: point the device at or above the span being planned first, and
check `ThroughputMeasurement::stream_sample_rate_hz` to confirm the path was
actually loaded. An unreachable server or an idle mission is an error,
never a rate — "0 MB/s" would condemn every span on the ladder.

The `HttpSource` block runs the same check passively on the stream it is
already reading, comparing the delivered IQ payload against the rate the
device reports in its packet headers, and warns once per configuration,
naming a narrower span from the device's own ladder that fits. The
verdict is also published as `StreamStats::link_budget` on the shared
stats handle, and is re-measured after a configuration restart.

### When a gap happens anyway: was it the device?

A gap has more than one cause, and the crate asks the device about the
one it can settle. On each stream-gap report `HttpSource` reads
`/healthstatus` and checks the
device block's own loss counters — errors, USB overflows and DSP
overflows, all per-second rates, so the reading describes the moment of
the gap. If they are nonzero the loss starts at the device and no amount
of network headroom will recover it. If they are zero the samples went
missing downstream, in the RTSA HTTP server's outbound buffer (it drops
past 8 MB) or on the wire. The reading is logged beside the gap warning
and published as `StreamStats::device_health`; the same read is available
directly:

```rust,no_run
# async fn health() -> sdr_aaronia_rs::Result<()> {
use sdr_aaronia_rs::http_endpoints::{AuthMethod, HttpEndpointsClient};

let client = HttpEndpointsClient::new("http://localhost:54664".into(), AuthMethod::None)?;
for block in client.get_device_health().await? {
    // `None` means the block reported none of the counters — the honest
    // answer is "cannot say", not a device that has been cleared.
    println!("{} -> losing nothing: {:?}", block.describe(), block.losing_nothing());
}
# Ok(())
# }
```

### What the device says it can do

The same control-plane surface declares the device's own limits, and
`get_device_capabilities()` reduces the two trees to them: model, serial
and firmware version, the bounds on centre frequency and reference
level, and the decimation ladder's depth.

```rust,no_run
# async fn caps() -> sdr_aaronia_rs::Result<()> {
use sdr_aaronia_rs::http_endpoints::{AuthMethod, HttpEndpointsClient};

let client = HttpEndpointsClient::new("http://localhost:54664".into(), AuthMethod::None)?;
let caps = client.get_device_capabilities().await;
println!("{:?} serial {:?}", caps.model, caps.serial);
if let Some(range) = caps.center_frequency_hz {
    println!("tunes {:.3}-{:.3} MHz", range.min / 1e6, range.max / 1e6);
}
// Highest first, and every rung one the device can actually be set to:
// its reported native rate is a measurement, so it is snapped to an
// exact ladder top before being halved down.
println!("sample rates: {:?}", caps.sample_rates());
# Ok(())
# }
```

Every field is optional and independently so, because the answer feeds
things that must not be guessed. The SoapySDR plugin publishes exactly
these at probe time — a V6 ECO declares 5.5 MHz–8 GHz and −55…+23 dBm —
and falls back per field for a device that cannot answer.

**Clearing the device does not mean you are alone on the server.** One
of the causes it leaves standing is another client: the HTTP Server block
serves each connection a full copy of the stream and refuses none, so a
second client doubles the server's egress. Nothing in the HTTP surface
counts connections, and the loss under saturation lands on an arbitrary
subset of the clients — measured, three of five ran contiguous while two
lost data — so a clean stream is not evidence of being alone either. For
a definitive count, look at the RTSA host. See `docs/HTTPSPEC.md`,
"Several clients on one server block".

## Reusable configuration profiles

Build your own configuration for specific bands:

```rust,no_run
use sdr_aaronia_rs::AaroniaConfig;

// UHF amateur band
let _config = AaroniaConfig::default()
    .center_frequency_hz(446.0e6)  // 446 MHz
    .sample_rate_hz(10.0e6)        // 10 MS/s (Fs)
    .reference_level_dbm(-30.0);   // -30 dBm

// 2m amateur band
let _config = AaroniaConfig::default()
    .center_frequency_hz(146.52e6)  // 2m amateur
    .sample_rate_hz(25e3)           // 25 kS/s (Fs)
    .reference_level_dbm(-30.0);    // -30 dBm
```

## Device control and monitoring

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
        frequency_center_hz: Some(162.4e6),
        frequency_span_hz: Some(25e3),
        reference_level_dbm: Some(-20.0),
        control_type: ControlType::Capture,
        ..Default::default()
    };
    client.configure_capture(config).await?;

    Ok(())
}
```

## FutureSDR integration

For existing [FutureSDR](https://github.com/FutureSDR/FutureSDR) users, the low-level block API integrates high-throughput streams (both RX and TX) into a flowgraph. `HttpSourceBuilder`, `HttpSinkBuilder`, and the corresponding blocks require the `futuresdr` feature:

```rust,ignore
use sdr_aaronia_rs::{HttpSourceBuilder, HttpSinkBuilder};
use futuresdr::runtime::Flowgraph;
use anyhow::Result;

fn main() -> Result<()> {
    // Low-level FutureSDR blocks
    let source = HttpSourceBuilder::new("http://127.0.0.1:54664")
        .center_frequency_hz(146.52e6)
        .sample_rate_hz(25e3)
        .build()?;

    let sink = HttpSinkBuilder::new("http://127.0.0.1:54664")
        .center_frequency_hz(433.0e6)
        .sample_rate_hz(1e6)
        .build()?;

    // Use in FutureSDR flowgraph
    let mut fg = Flowgraph::new();
    let _src = fg.add_block(source);
    let _sink = fg.add_block(sink);
    // ... connect to other blocks

    Ok(())
}
```

## Streaming with authentication

The low-level `HttpSourceBuilder` (also part of the `futuresdr` feature) offers advanced properties (e.g., `buffer_size`, `timeout_ms`, `rate_reduction`) and authentication settings:

```rust,ignore
use sdr_aaronia_rs::{AuthMethod, HttpSourceBuilder, StreamFormat};
use anyhow::Result;

fn main() -> Result<()> {
    let _advanced_source = HttpSourceBuilder::new("http://127.0.0.1:54664")
        .center_frequency_hz(446.125e6)  // UHF band
        .sample_rate_hz(12.5e3)          // 12.5 kS/s
        .format(StreamFormat::Int16)     // High-performance format
        .auth(AuthMethod::Basic {
            username: "admin".to_string(),
            password: "secure_pass".to_string(),
        })
        .input("main")                  // Specific input stream
        .rate_reduction(4)              // Time compression; no effect on IQ
        .buffer_size(16384)             // Large buffer
        .timeout_ms(1000)               // Fast timeout
        .build()?;
    Ok(())
}
```

## Low-level asynchronous stream reading

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

## Additional device management

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

## Environment variables

| Variable | Effect |
|---|---|
| `AARONIA_SDK_PATH` | Overrides the RTSA-Suite installation directory used for SDK / `RTSAFileTool` detection. Works on every platform. On macOS — which has no default install path and no native SDK build — it only affects `RTSAFileTool` and XML-config detection. |
| `AARONIA_USER_AGENT` | Overrides the HTTP `User-Agent` string sent by every outbound request (default: `sdr-aaronia-rs/<version>`). |

## Runnable examples

The programs in `examples/` cover the same ground as this document, as
code you can run. CI builds them, so they stay current:

| Task | Example |
| --- | --- |
| HTTP IQ streaming, first samples | [`http_iq_quickstart.rs`](../examples/http_iq_quickstart.rs) |
| Health checks, server info, input enumeration | [`device_control.rs`](../examples/device_control.rs) |
| Frequency hopping via the `sdr-source` traits | [`channel_hopping.rs`](../examples/channel_hopping.rs) |
| FutureSDR flowgraph with FM demodulation | [`noaa_scanner.rs`](../examples/noaa_scanner.rs) |
| Native SDK capture and transmit | [`native_sdk_basic.rs`](../examples/native_sdk_basic.rs), [`native_sdk_transmit.rs`](../examples/native_sdk_transmit.rs) |
| RTSA file playback and metadata inspection | [`read_rtsa_file.rs`](../examples/read_rtsa_file.rs), [`dump_metadata.rs`](../examples/dump_metadata.rs) |
| Python (NumPy and Arrow), SoapySDR from Python | [`python_arrow_example.py`](../examples/python_arrow_example.py), [`soapy_python_example.py`](../examples/soapy_python_example.py) |

```bash
# args: <center-hz> <sample-rate-hz> <url>
cargo run --example http_iq_quickstart --features http -- 2440e6 15.36e6 http://localhost:54664
```

## Related

- [QUICKSTART.md](QUICKSTART.md) — RTSA-Suite mission setup and troubleshooting.
- [APPS.md](APPS.md) — SDR++, GQRX, GNU Radio and SoapySDR from Python.
- [HTTPSPEC.md](HTTPSPEC.md) — the RTSA HTTP API this crate speaks.
