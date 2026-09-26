# sdr-aaronia-rs

> **Moved to [Specola](https://github.com/isaacbentley/specola).** This repository now lives at
> [`crates/sdr-aaronia`](https://github.com/isaacbentley/specola/tree/main/crates/sdr-aaronia) (package `sdr-aaronia-rs`), with its full
> history; its tags there carry a prefix. Development continues there, and this repository is
> no longer updated.

[![Crates.io](https://img.shields.io/crates/v/sdr-aaronia-rs.svg)](https://crates.io/crates/sdr-aaronia-rs)
[![Docs.rs](https://docs.rs/sdr-aaronia-rs/badge.svg)](https://docs.rs/sdr-aaronia-rs)
[![CI](https://github.com/isaacbentley/sdr-aaronia-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/isaacbentley/sdr-aaronia-rs/actions/workflows/ci.yml)
[![License: GPL-3.0-or-later](https://img.shields.io/github/license/isaacbentley/sdr-aaronia-rs.svg)](https://choosealicense.com/licenses/gpl-3.0/)

One API for Aaronia SPECTRAN analyzers, whether the samples
come from the native SDK, an RTSA-Suite HTTP server, or a recorded file.
Python bindings and a SoapySDR plugin come from the same engine.

*Disclaimer: This project is not affiliated with Aaronia AG. Aaronia, SPECTRAN, and RTSA-Suite PRO are trademarks of Aaronia AG.*

Working with a SPECTRAN usually means choosing a transport first and
then writing against whatever API that transport exposes. `SpectranSource`
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
- **Says whether the link can carry the span** before a capture is
  wasted: `link_budget` measures the path end to end and names the
  widest span on the device's decimation ladder that fits it.

Three backends feed one engine that a range of consumers read from:

```text
               ┌── Native SDK (C FFI)
Backends:      ├── HTTP Streaming (REST + binary chunked)
               └── Offline .rtsa Files (binary parser)
                       │
                       ▼
Engine:        [ SpectranSource / Unified Source ]
                       │
                       ▼
Consumers:     ├── Native Rust API
               ├── FutureSDR Block (HttpSource / HttpSink)
               ├── Seify Driver (seify_impl.rs)
               ├── C ABI (c_api.rs) ──► SoapySDR (C++) ──► GQRX / SDR++ / GNU Radio
               └── Python Bindings (PyO3: python-aaronia) ──► NumPy / Arrow
```

## Link requirements

The server streams IQ at a fixed number of bytes per sample, so the span
you ask for sets a byte rate the whole path has to sustain. Miss it and the
*server* drops what it cannot send — unsignalled gaps that look fine in a
waterfall and defeat any digital demodulator.

| Real-time bandwidth | Sample rate | Needs (4 B/sample) | Link |
|---|---|---|---|
| up to 12.2 MHz | 15.36 MS/s | 61.4 MB/s | gigabit |
| up to 24.5 MHz | 30.72 MS/s | 122.9 MB/s | gigabit measured 1024 skips; prefer 2.5GbE |
| up to 49.1 MHz | 61.44 MS/s | 245.8 MB/s | **2.5GbE** |

**An ECO 100 needs 2.5GbE.** Its 44 MHz of real-time bandwidth only fits on
the top rung, which costs 245.8 MB/s — past gigabit, and close to the
292 MB/s a 2.5GbE path measured. That leaves no room for an 8-byte wire
format: stay on `int16` or `float16`.

Treat the table as a starting point. `link_budget` measures *your* path end
to end and names the widest span that fits.

## HTTP sample conversion

Float16 IQ payloads use `half`'s bulk conversion, which selects SIMD at
runtime on supported CPUs (AArch64 FP16 or x86 F16C) and retains a portable
fallback. On little-endian targets, aligned payloads are borrowed directly;
odd-aligned payloads pass through a 1 KiB stack buffer. Big-endian targets
retain scalar little-endian decoding. Tests compare every half-float bit
pattern with the preceding scalar implementation, including subnormals,
signed zeros, and NaN payloads, at both alignments and with odd IQ counts.

On the local macOS ARM64 host, release-mode conversion time fell as follows.
Each result compares median elapsed times from three alternating A/B rounds,
with 20,000 conversions per round after 2,000 warm-up calls:

| IQ pairs per payload | Aligned payload | Odd-aligned payload |
| --- | ---: | ---: |
| 49,152 | 35.6% less time | 9.5% less time |
| 65,535 | 36.3% less time | 9.8% less time |

These measurements include output allocation, but exclude HTTP framing,
networking, and application DSP; they are not Raspberry Pi measurements.
This change applies to Float16 IQ, not the default Int16 capture format.
Run the same comparison without concurrent builds or other benchmarks:

```bash
cargo test --release --lib --no-default-features --features http \
  float16_bulk_throughput_meter -- --ignored --nocapture
```

## Stream continuity

A source hands its consumer a flat run of samples, and that run *claims* to be
continuous. Six things break the claim, and `HttpSource` knows about all six:

| event | cause |
| --- | --- |
| server gap | the RTSA server dropped what it could not send; seen as a jump between one packet's `endTime` and the next's `startTime` larger than a tolerance measured from the stream (half a packet, or above the observed timestamp jitter, capped at 1 ms — see `DropDetector`; not yet verified live at high rates) |
| backward timestamp | a packet's `startTime` precedes the previous packet's `endTime` by more than the same tolerance: nothing is missing, but the two do not follow one another |
| undecodable packet | an IQ packet, or one whose header could not be read, framed but could not be decoded; it is skipped and its neighbours kept, and the hole is a gap where it sat |
| capacity trim | the buffer reached its memory bound and the oldest samples were discarded. The trim sits one fetch sweep above the refill target, so a consumer that falls behind backs up the socket instead, and any loss then shows as a server gap; it is reached only by a JSON stream, whose samples have no fixed width, or if the headroom's assumptions — chunk and packet sizes, each learned before it is parsed — stop holding |
| reconnect | the stream ended, errored, or delivered nothing for 5 s, and was reopened; the parser resets and the drop detector forgets the last timestamp (keeping its jitter calibration) |
| retune | the device's centre frequency or sample rate changed mid-stream |

`HttpSource` outputs IQ only. Spectra, histogram and category packets on a
mixed mission are skipped and counted in `StreamStats::non_iq_packets_skipped`;
they feed neither the output, the drop detector nor the frequency tracking. That
includes the ones the parser cannot decode — a compressed spectra packet carries
no byte length to frame it by — which cost no IQ and so mark no gap.

Counters for these have always been available through `StreamStats`. What a
counter cannot say is *which* samples belong to the old epoch — and that is the
only thing a consumer can act on, because symbol timing, carrier tracking and
burst framing are all carried across the boundary between one delivery and the
next. `link_budget`'s own warning applies here: every dropped sample is an
unsignalled phase discontinuity that breaks digital symbol lock.

`with_stream_breaks` reports each one against the **absolute index of the first
sample after it**, counted in this source's output stream:

```rust,no_run
# // `HttpSourceBuilder` is the FutureSDR block: compiled only with that feature.
# #[cfg(feature = "futuresdr")]
# fn main() -> Result<(), sdr_aaronia_rs::Error> {
use std::sync::Arc;
use sdr_aaronia_rs::{HttpSourceBuilder, RecordingBreakSink, StreamDiscontinuity};

let breaks = Arc::new(RecordingBreakSink::new());
let source = HttpSourceBuilder::new("http://atc.local:54664")
    .center_frequency_hz(146.52e6)
    .sample_rate_hz(1e6)
    .with_stream_breaks(breaks.clone())
    .build()?;

// …later, after the flowgraph has run:
for (at_sample, cause) in breaks.breaks() {
    match cause {
        StreamDiscontinuity::Gap => println!("reset decoder state at {at_sample}"),
        StreamDiscontinuity::Initial { center_hz, rate_hz }
        | StreamDiscontinuity::Retune { center_hz, rate_hz } => {
            println!("samples from {at_sample} are {center_hz} Hz at {rate_hz} Sa/s")
        }
    }
}
# Ok(())
# }
# #[cfg(not(feature = "futuresdr"))]
# fn main() {}
```

Implement `StreamBreakSink` yourself to push into whatever queue the consumer
already reads; `RecordingBreakSink` is the built-in one, for tests and for
looking at a finished run.

Two properties make the index usable. A break is recorded **before** the
samples it precedes are published, so by the time a consumer can see sample
`i`, every break at or before `i` is already in the sink. And samples the
source discards — trimmed, or cleared on a reconnect — never occupy an index,
so the coordinate is simply how many samples the block has produced. Breaks are
held against the samples still queued and emitted only as those are produced,
which is what makes a reported index final: a trim that arrives later rebases
what is still pending rather than invalidating what was already said.

A `Retune` is measured against the geometry **last announced**, not against the
previous packet. A baseline that moved with every packet would let a device
creeping by less than a tolerance per packet travel arbitrarily far without
ever reporting a change, since each step would be judged against the step
before it rather than against what the consumer believes. Centre moves of more
than 1 Hz and rate moves outside the crate's 10% band count; the rate ladder
steps in powers of two, so a real change clears that band by a wide margin
while a rate the parser had to infer from `samples / duration` wobbles well
inside it. A packet declaring no usable rate at all is ignored for this
purpose rather than taken as a new baseline. A restart that clears the buffer
also discards any announcement still queued in it, so the baseline goes back to
the last geometry actually *reported* — a retune the restart swallowed is then
announced after it.

`Initial` is emitted at sample 0 of every run, whether or not it differs from
what was requested. The server serves the nearest rung of its own rate ladder
and the span its mission is configured for, so the first packet is exactly
where a disagreement with the request appears — and a consumer comparing
packets only against each other would never see it. Breaks at one index are
reported as one boundary — at most a gap, then at most one geometry — so when
a trim before the first sample discards the band the run opened on, the
consumer gets a single `Initial` naming the geometry its first sample actually
arrived under.

## Installation

Add the following to your `Cargo.toml`:

```toml
[dependencies]
# By default, includes HTTP, File, native sdr-source trait, and C FFI backend support
sdr-aaronia-rs = "0.8"
tokio = { version = "1.43", features = ["rt-multi-thread", "macros"] }

# To enable additional backends, opt into their features (e.g. native-sdk, futuresdr)
# sdr-aaronia-rs = { version = "0.8", features = ["native-sdk", "futuresdr"] }
```

HTTP reads use one timeout budget for the entire requested block and return partial data at a timeout, frequency change, sample-rate change, or detected gap. `capture_frequency_hz()` and `capture_sample_rate_hz()` describe the returned samples even when newer packets are queued. File tuning setters preserve the recording's metadata.

## Quickstart

Set the RF parameters and read:

```rust,no_run
use sdr_aaronia_rs::{SpectranSource, SpectranConfig};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let config = SpectranConfig::default()
        .center_frequency_hz(446.0e6)  // 446 MHz
        .sample_rate_hz(10.0e6)        // 10 MS/s (Fs), not RF bandwidth
        .reference_level_dbm(-30.0);   // -30 dBm

    let mut source = SpectranSource::new(config).await?;

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

with aaronia.open(
    "http://localhost:54664", center_frequency_hz=2.44e9, bandwidth_hz=10e6
) as src:
    for block in src.blocks(65536):   # numpy complex64 arrays
        process(block)
```

Reads land in NumPy or PyArrow with one copy out of the receive buffer.
Blocking calls release the GIL, errors arrive as typed exceptions, and
the package ships type stubs. Wheels are abi3 for CPython 3.9 and
later. The bundled `aaronia-doctor` command checks a server and names
the fix for whatever is wrong. Full reference:
[python-aaronia/README.md](python-aaronia/README.md).

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
`SpectranSeifyDevice::from_args`. It is not part of seify's built-in
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

`SpectranConfig::read_timeout` (default 30 s) bounds `read_samples`.
`read_samples_deadline`, and therefore the SoapySDR and seify paths, uses
its caller's per-call deadline instead.

A dropped HTTP stream, from an RTSA restart or a network interruption,
reconnects automatically. This is `SpectranConfig::auto_reconnect`,
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
| `sdr-source` | Integrates `SpectranSdrSource` implementing the native `SdrSource` traits. | **Yes** |
| `ffi` | Builds the C-API export layer. | **Yes** |

## Testing & Contributing

The test suite consists of unit tests, integration tests against LFS captures, and property tests enforcing specification invariants.

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for detailed instructions on running the test suite, generating coverage reports, and formatting your code before submitting a Pull Request.

## Documentation

Start here:

- [Quickstart](docs/QUICKSTART.md) — configuring an RTSA-Suite mission, first samples in Rust, Python and SoapySDR, and troubleshooting for common setup failures.
- [Usage](docs/USAGE.md) — worked examples for each part of the API, plus the `AARONIA_SDK_PATH` and `AARONIA_USER_AGENT` environment variables.
- [Using existing SDR apps](docs/APPS.md) — SDR++, GQRX, GNU Radio, SoapySDR from Python.
- [Synchronising receivers](docs/SYNC.md) — what the hardware can and cannot do for multi-device timing, and the recipe that works.

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
