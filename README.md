# sdr-aaronia-rs

[![Crates.io](https://img.shields.io/crates/v/sdr-aaronia-rs.svg)](https://crates.io/crates/sdr-aaronia-rs)
[![Docs.rs](https://docs.rs/sdr-aaronia-rs/badge.svg)](https://docs.rs/sdr-aaronia-rs)
[![CI](https://github.com/isaacbentley/sdr-aaronia-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/isaacbentley/sdr-aaronia-rs/actions/workflows/ci.yml)
[![License: GPL-3.0-or-later](https://img.shields.io/github/license/isaacbentley/sdr-aaronia-rs.svg)](https://choosealicense.com/licenses/gpl-3.0/)

One API for Aaronia SPECTRAN analyzers and SDRs, whether the samples
come from the native SDK, an RTSA-Suite HTTP server, or a recorded file.
Python bindings and a SoapySDR plugin come from the same engine.

*Disclaimer: This project is not affiliated with Aaronia AG. Aaronia, SPECTRAN, and RTSA-Suite PRO are trademarks of Aaronia AG.*

Working with a SPECTRAN usually means choosing a transport first and
then writing against whatever API that transport exposes. `AaroniaSource`
removes the choice: point it at a file, a URL, or nothing at all, and it
selects a backend and presents the same interface either way.

| What you configure | What it uses |
| --- | --- |
| `file_path` | Buffered playback of an RTSA capture file |
| `http_base_url` | HTTP streaming from an RTSA-Suite server block |
| Neither | The native SDK, falling back to `localhost:54664` |
| `force_source_type` | Exactly the backend you name |

## What it does

- **Streams IQ and spectra over HTTP** in JSON, Int16, Float16 or
  Float32, with Basic and token authentication. Retuning mid-stream
  needs no Aaronia licence, and dropped streams reconnect on their own.
- **Reads `.rtsa` capture files** through buffered I/O, with metadata
  extraction and multi-stream support.
- **Talks to the hardware directly** through the Aaronia SDK on Windows
  and Linux, including transmit. The SDK is not available for macOS.
- **Controls and monitors the device**: streaming parameters, health,
  input enumeration and the configuration tree.
- **Plugs into FutureSDR** with `HttpSource` and `HttpSink` flowgraph
  blocks.

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

Set the RF parameters and read:

```rust,no_run
use sdr_aaronia_rs::{AaroniaSource, AaroniaConfig};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AaroniaConfig::default()
        .center_frequency(446.0e6)     // 446 MHz
        .span_frequency(10.0e6)        // 10 MHz span
        .reference_level(-30.0);       // -30 dBm

    let mut source = AaroniaSource::new(config).await?;

    let mut buffer = Vec::with_capacity(1024);
    let n = source.read_samples(&mut buffer, 1024).await?;
    println!("Received {} IQ samples", n);

    Ok(())
}
```

## Usage

[docs/USAGE.md](docs/USAGE.md) has worked examples for everything the
quickstart leaves out: the builder pattern, explicit backend selection,
wire formats and network bandwidth, configuration profiles, device
control, FutureSDR integration, authentication and low-level stream
access. Its Rust snippets are compiled as doctests, and it indexes the
runnable programs in [`examples/`](examples/).

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
cfg.sample_rate = 15.36e6

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
