# sdr-aaronia-rs

[![Crates.io](https://img.shields.io/crates/v/sdr-aaronia-rs.svg)](https://crates.io/crates/sdr-aaronia-rs)
[![Docs.rs](https://docs.rs/sdr-aaronia-rs/badge.svg)](https://docs.rs/sdr-aaronia-rs)
[![CI](https://github.com/isaacbentley/sdr-aaronia-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/isaacbentley/sdr-aaronia-rs/actions/workflows/ci.yml)
[![License: GPL-3.0-or-later](https://img.shields.io/github/license/isaacbentley/sdr-aaronia-rs.svg)](https://choosealicense.com/licenses/gpl-3.0/)

Unified Rust interface for Aaronia Spectran Spectrum Analyzers / SDRs, featuring Python bindings, a SoapySDR plugin, HTTP streaming, and native SDK support.

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

*Note: writes to the HTTP `/remoteconfig` endpoint require a separate Aaronia "Remote Config" license; capture control via `/control`, including retuning, does not. One server behaviour to be aware of: `/control` applies a frequency change only when `frequencyCenter` and `frequencySpan` are both present. A request carrying one of them returns `{"success":true}` and is ignored. The crate always sends the complete tuple.*

## Installation

Add the following to your `Cargo.toml`:

```toml
[dependencies]
# By default, includes HTTP, File, native sdr-source trait, and C FFI backend support
sdr-aaronia-rs = "0.6"
tokio = { version = "1.43", features = ["rt-multi-thread", "macros"] }

# To enable additional backends, opt into their features (e.g. native-sdk, futuresdr)
# sdr-aaronia-rs = { version = "0.6", features = ["native-sdk", "futuresdr"] }
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

## Usage

The Quickstart above covers the unified API: set RF parameters, read
samples. [docs/USAGE.md](docs/USAGE.md) has worked examples for the rest:
the builder pattern, explicit backend selection, wire formats and network
bandwidth, configuration profiles, device control, FutureSDR integration,
authentication, and low-level stream access. Its Rust snippets are
compiled as doctests.

The programs in `examples/` cover the same ground as runnable code, and
are built by CI:

| Task | Example |
| --- | --- |
| HTTP IQ streaming, first samples | [`http_iq_quickstart.rs`](examples/http_iq_quickstart.rs) |
| Health checks, input enumeration, recording control, license probing | [`device_control.rs`](examples/device_control.rs) |
| Frequency hopping via the `sdr-source` traits | [`channel_hopping.rs`](examples/channel_hopping.rs) |
| FutureSDR flowgraph with FM demodulation | [`noaa_scanner.rs`](examples/noaa_scanner.rs) |
| Native SDK capture and transmit | [`native_sdk_basic.rs`](examples/native_sdk_basic.rs), [`native_sdk_transmit.rs`](examples/native_sdk_transmit.rs) |
| RTSA file playback and metadata inspection | [`read_rtsa_file.rs`](examples/read_rtsa_file.rs), [`dump_metadata.rs`](examples/dump_metadata.rs) |
| Python (NumPy and Arrow), SoapySDR from Python | [`python_arrow_example.py`](examples/python_arrow_example.py), [`soapy_python_example.py`](examples/soapy_python_example.py) |

```bash
# args: <center-hz> <sample-rate-hz> <url>
cargo run --example http_iq_quickstart --features http -- 2440e6 12.288e6 http://localhost:54664
```

[docs/QUICKSTART.md](docs/QUICKSTART.md) covers configuring the
RTSA-Suite HTTP Server block, which all of the above depends on.

## Using it from other tools

The Rust crate is the engine. The same code drives three other
surfaces, so an Aaronia device works in the tools people already use.

### Python

```bash
pip install python-aaronia
```

```python
import aaronia

cfg = aaronia.AaroniaConfig()
cfg.http_base_url = "http://localhost:54664"
cfg.center_freq = 2.44e9
cfg.sample_rate = 12.288e6

src = aaronia.AaroniaSource()
src.start_streaming(cfg)
samples = src.read_samples_numpy(65536)   # numpy complex64
src.stop_streaming()
```

Reads land in NumPy or PyArrow with one copy out of the receive buffer.
Blocking calls release the GIL, errors arrive as typed exceptions, and
the package ships type stubs. Wheels are abi3 for CPython 3.9 and
later. Full reference: [python-aaronia/README.md](python-aaronia/README.md).

### SoapySDR: GQRX, SDR++, GNU Radio and others

```bash
SoapySDRUtil --probe="driver=aaronia,url=http://localhost:54664"
```

Every release attaches a prebuilt plugin for Linux, macOS and Windows,
so no toolchain is needed. The plugin streams CF32 and CS16, honours
`timeoutUs` with partial reads, stays safe to retune while streaming,
and reports timestamps and dropped-block counts.

Per-application setup is in [docs/APPS.md](docs/APPS.md). Installation,
building from source and the wire-format trade-offs are in
[PLUGINS.md](PLUGINS.md); pass `format=I16` to halve network bandwidth,
which is a real wire-format change rather than a client-side conversion.

### seify (Rust-native)

Enable the `seify` feature and construct the device with
`AaroniaSeifyDevice::from_args`. It is not part of seify's built-in
enumeration, so it will not appear in `seify::enumerate()`. See
[PLUGINS.md](PLUGINS.md).

### What has been tested

Not every path has run against hardware. Transmit, dual-channel capture
and the native-SDK backend have not.
[docs/VERIFICATION.md](docs/VERIFICATION.md) gives the status of each
feature and how it was checked.

## Connection Resilience

Connecting (the `/info` probe and initial tuning PUT) retries transient
failures up to 4 times with exponential backoff, bounded by a 10 second
total budget. Refused connections, unresolved DNS and 5xx/408/429
responses are retried; 4xx and configuration errors fail on the first
attempt. This matters for `*.local` hostnames, which refuse the first
connection from a cold process while mDNS resolves.

`AaroniaConfig::read_timeout` (default 30 s) bounds `read_samples`.
`read_samples_deadline`, and therefore the SoapySDR and seify paths, uses
its caller's per-call deadline instead.

A dropped HTTP stream, from an RTSA restart or a network interruption,
reconnects automatically. This is `AaroniaConfig::auto_reconnect`,
enabled by default. The reader reopens the stream, re-applies the
current tuning (a restarted server returns to its mission's frequency),
and flags the first packet after the gap as an overrun so callers know
samples were missed. After 5 attempts, roughly 8 seconds of backoff, it
stops and reads report a closed stream, matching the behaviour of
`auto_reconnect(false)`.

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

## Testing & Contributing

The test suite consists of unit tests, integration tests against LFS captures, and property tests enforcing specification invariants.

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for detailed instructions on running the test suite, generating coverage reports, and formatting your code before submitting a Pull Request.

## Documentation

Start here:

- [Quickstart](docs/QUICKSTART.md) — configuring an RTSA-Suite mission, first samples in Rust, Python and SoapySDR, and troubleshooting for common setup failures.
- [Usage](docs/USAGE.md) — worked examples for each part of the API, plus the `AARONIA_SDK_PATH` and `AARONIA_USER_AGENT` environment variables.
- [Using existing SDR apps](docs/APPS.md) — SDR++, GQRX, GNU Radio, SoapySDR from Python.

Reference:

- [Verification status](docs/VERIFICATION.md) — which features have been tested against real hardware.
- [SDR plugins](PLUGINS.md) — SoapySDR and seify setup, wire-format trade-offs, metrics.
- [Architecture & Design](DESIGN.md) — Internal architecture and execution flow.
- [RTSA File Format Specification](docs/FILESPEC.md) — On-disk `.rtsa` capture-file format and how this crate parses it.
- [HTTP API Specification](docs/HTTPSPEC.md) — The RTSA HTTP streaming and control API, including Remote Config licence detection.
- [Native SDK Specification](docs/SDKSPEC.md) — The Aaronia RTSA-Suite PRO SDK surface and the Rust binding notes.
- [Changelog](CHANGELOG.md) — Release history.

## License

This project is licensed under the GNU General Public License v3.0 or later (GPL-3.0-or-later) - see the LICENSE file for details.
