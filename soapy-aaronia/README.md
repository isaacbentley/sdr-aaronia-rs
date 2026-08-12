# soapy-aaronia

A [SoapySDR](https://github.com/pothosware/SoapySDR) module for Aaronia
SPECTRAN V6 devices, backed by the
[`sdr-aaronia-rs`](https://github.com/isaacbentley/sdr-aaronia-rs) crate's
C API. The Rust library is **statically linked** into the module — the
shipped `libaaroniaSupport` has no Rust runtime dependency to install.

Works with any SoapySDR application (GQRX, SDR++, GNU Radio's Soapy
blocks, `SoapySDRUtil`, the SoapySDR Python bindings).

## Build

Requirements: CMake ≥ 3.14, a C++17 compiler, SoapySDR ≥ 0.7 with dev
headers (`libsoapysdr-dev` / `brew install soapysdr` / vcpkg `soapysdr`),
and a Rust toolchain — CMake drives `cargo build --release` itself.

```bash
cmake -S soapy-aaronia -B soapy-aaronia/build -DCMAKE_BUILD_TYPE=Release
cmake --build soapy-aaronia/build
```

Try it without installing:

```bash
export SOAPY_SDR_PLUGIN_PATH=$PWD/soapy-aaronia/build
SoapySDRUtil --check=aaronia
SoapySDRUtil --probe="driver=aaronia,url=http://localhost:54664"
```

Install into SoapySDR's module directory:

```bash
sudo cmake --install soapy-aaronia/build
```

## Device arguments

| Arg | Meaning |
| --- | --- |
| `url` | RTSA-Suite HTTP server URL (default `http://localhost:54664`) |
| `file` | Play back a recorded `.rtsa` file |
| `serial` | Select a device by serial via the native-SDK backend (Windows/Linux with the Aaronia SDK; omit `url` to allow SDK auto-detection) |
| `freq` / `rate` / `ref_level` | Initial center frequency, sample rate, reference level |
| `format` | HTTP **wire** format; `format=I16` (optionally `scale=N`) is the genuine low-bandwidth network mode |
| `rx_channel` | `Rx1` (default), `Rx2`, or `Rx1And2` (native SDK, full V6 only) |
| `read_timeout` | Seconds the crate's own blocking reads wait (default 30). `readStream` always uses SoapySDR's per-call `timeoutUs`, so this rarely matters here |

```python
import SoapySDR
sdr = SoapySDR.Device("driver=aaronia,url=http://atc.local:54664,format=I16")
```

## Streams

- **RX:** `CF32` (native) and `CS16`. Note the distinction: the
  app-side stream format is a client-side conversion; only the `format=`
  device arg changes what crosses the network.
- `readStream` honours `timeoutUs` and returns partial reads within the
  deadline, per the SoapySDR contract. Retuning while streaming is safe
  (fully serialized against the reader) and needs no Aaronia license:
  the plugin retunes through the RTSA `/control` endpoint, always
  sending center frequency and span together — RTSA servers silently
  ignore capture requests that carry only one of the two (live-verified
  against RTSA-Suite PRO with a SPECTRAN V6 ECO).
- **TX:** `CF32`, single channel, available only when the module is
  built against the native SDK on Windows/Linux — elsewhere
  `setupStream(TX)` fails with a descriptive error. Bursts are pushed
  for immediate transmission; timed TX (`SOAPY_SDR_HAS_TIME`) is not
  supported. **The entire TX path is hardware-unverified.**

## Time, gain, sensors

- `hasHardwareTime("GPS")` probes truthfully: it is only true on the
  native-SDK backend with a valid GPS fix. `getHardwareTime("GPS")`
  returns epoch nanoseconds (integer-domain conversion).
- The single gain element **`REF` is the Aaronia reference level in
  dBm** — *raising* it reduces sensitivity; it is not an amplifier gain.
- `readSensor("cumulative_drops")` reports server-side dropped blocks.

## Known limitations

- One RX channel through the plugin (`rx_channel=Rx2` selects the second
  antenna input; true dual-channel reads are available via the crate's
  Rust/Python/C APIs, not the Soapy streaming interface).
- Enumeration advertises a default localhost candidate without probing
  it (`find()` must not block on the network).
