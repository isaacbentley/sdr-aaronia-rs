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

## Hardware verification status

Not every code path has been exercised against hardware. The
development device is a SPECTRAN V6 ECO with a single RX channel and no
TX licence, driven through RTSA-Suite PRO over HTTP from macOS. Paths
requiring a second RX input, a transmitter, or the Windows/Linux native
SDK are marked unverified.

| Capability | Backend | Status |
| --- | --- | --- |
| IQ streaming, all four wire formats (F32 / F16 / I16 / JSON) | HTTP | **Live-verified** |
| Spectra streaming | HTTP | **Live-verified** |
| Mid-stream retuning (centre, span) | HTTP | **Live-verified** |
| Mid-stream reference-level change | HTTP | Confirmed manually against the device; no automated live assertion |
| Auto-reconnect after a dropped stream | HTTP | Streaming live-verified; the drop-and-recover path is mock-tested |
| Drop/overrun detection, rate reduction, `scale=N` | HTTP | **Live-verified** |
| Long-run stability (>120 s continuous) | HTTP | **Live-verified** |
| Connect retry | HTTP | Mock-tested; the mDNS race it addresses did not reproduce on demand |
| `.rtsa` playback and metadata | File | **Verified against real captures**, byte-compared with the official format specification |
| seify backend | HTTP | **Live-verified** |
| SoapySDR plugin RX | HTTP | Verified manually (~9.7 Msps via `SoapySDRUtil`); no automated live test, as a `soapysdr` dev-dependency would make `cargo test` unbuildable without system SoapySDR |
| Python bindings RX | HTTP | Verified manually (NumPy and Arrow); no automated live test |
| TX (`UnifiedSink`, `aaronia_sink_*`, SoapySDR TX) | Native SDK | **Hardware-unverified**. No TX-licensed device available |
| Dual-channel RX (`Rx1And2`, `read_samples_dual`) | Native SDK | **Hardware-unverified**. Requires a full V6 |
| GPS hardware time | Native SDK | **Hardware-unverified** |
| Native SDK capture generally | Native SDK | **Hardware-unverified**; compiled and unit-tested in a Linux VM each release |
| HTTP TX push (`/sample`) | HTTP | Endpoint exercised live; RF output not measured |

"Live-verified" means an `#[ignore]`d test in
[`tests/live_smoke.rs`](tests/live_smoke.rs) asserts the behaviour
against hardware, and is reproducible by anyone with a device. Entries
marked "verified manually" were observed working but have no automated
assertion and can regress without detection. Run the automated set
with:

```bash
cargo test --all-features --test live_smoke -- --ignored --nocapture
```

Contributions that convert an unverified row, particularly from users
with a full V6 or a TX licence, are welcome.

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

## Testing & Contributing

The test suite consists of unit tests, integration tests against LFS captures, and property tests enforcing specification invariants.

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for detailed instructions on running the test suite, generating coverage reports, and formatting your code before submitting a Pull Request.

## Documentation

Start here:

- [Quickstart](docs/QUICKSTART.md) — configuring an RTSA-Suite mission, first samples in Rust, Python and SoapySDR, and troubleshooting for common setup failures.
- [Usage](docs/USAGE.md) — worked examples for each part of the API.
- [Using existing SDR apps](docs/APPS.md) — SDR++, GQRX, GNU Radio, SoapySDR from Python.

Reference:

- [Architecture & Design](DESIGN.md) — Internal architecture and execution flow.
- [RTSA File Format Specification](docs/FILESPEC.md) — On-disk `.rtsa` capture-file format and how this crate parses it.
- [HTTP API Specification](docs/HTTPSPEC.md) — The RTSA HTTP streaming and control API.
- [Native SDK Specification](docs/SDKSPEC.md) — The Aaronia RTSA-Suite PRO SDK surface and the Rust binding notes.
- [Changelog](CHANGELOG.md) — Release history.

## License

This project is licensed under the GNU General Public License v3.0 or later (GPL-3.0-or-later) - see the LICENSE file for details.
