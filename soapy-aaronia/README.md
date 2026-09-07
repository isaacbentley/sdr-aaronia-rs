# soapy-aaronia

A [SoapySDR](https://github.com/pothosware/SoapySDR) module for Aaronia
SPECTRAN V6 devices, backed by the
[`sdr-aaronia-rs`](https://github.com/isaacbentley/sdr-aaronia-rs) crate's
C API. The Rust library is statically linked into the module, so the
shipped `libaaroniaSupport` has no Rust runtime dependency.

Works with any SoapySDR application: GQRX, SDR++, GNU Radio's Soapy
blocks, `SoapySDRUtil` and the SoapySDR Python bindings.
[docs/APPS.md](../docs/APPS.md) covers per-application setup.

## Install a prebuilt module

Each [release](https://github.com/isaacbentley/sdr-aaronia-rs/releases)
attaches a built module per platform, so neither CMake nor a Rust
toolchain is required. Download the archive for your system and unpack
it:

| Platform | Archive |
| --- | --- |
| Linux x86-64 | `SoapyAaronia-<version>-linux-x86_64.tar.gz` |
| macOS (Apple silicon) | `SoapyAaronia-<version>-macos-arm64.tar.gz` |
| Windows x86-64 | `SoapyAaronia-<version>-windows-x86_64.zip` |

The Linux module is built on Ubuntu 24.04 and needs glibc 2.38 or
later, so it does not load on Ubuntu 22.04 or Debian 12. Build from
source on those.

Each archive contains the module, an installer, these instructions as
`INSTALL.md`, and the licence. Run the installer from the unpacked
directory:

```bash
./install.sh
```

On Windows, in PowerShell:

```powershell
.\install.ps1
```

It locates SoapySDR's module directory, clears the macOS quarantine
flag, copies the module in, and confirms it loads. If SoapySDR is not
installed or its module directory cannot be found, the installer prints
what to do by hand instead of guessing.

To install by hand, or to use the module without administrator rights,
point `SOAPY_SDR_PLUGIN_PATH` at the unpacked directory:

```bash
xattr -d com.apple.quarantine libaaroniaSupport.so   # macOS only
export SOAPY_SDR_PLUGIN_PATH=/path/to/unpacked
SoapySDRUtil --check=aaronia
```

Applications that enumerate plugins at startup, such as SDR++ and GQRX,
must be launched after `SOAPY_SDR_PLUGIN_PATH` is set.

## Build from source

Requirements: CMake ≥ 3.14, a C++17 compiler, SoapySDR ≥ 0.7 with dev
headers (`libsoapysdr-dev` / `brew install soapysdr` / vcpkg `soapysdr`),
and a Rust toolchain. CMake invokes `cargo build --release` itself.

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
| `freq` / `rate` / `ref_level` | Initial center frequency, sample rate, reference level. `rate` snaps to the device's ladder — see [Sample rates](#sample-rates) |
| `format` | HTTP wire format. `format=I16`, optionally with `scale=N`, is the low-bandwidth network mode |
| `rx_channel` | `Rx1` (default), `Rx2`, or `Rx1And2` (native SDK, full V6 only) |
| `read_timeout` | Seconds the crate's own blocking reads wait (default 30). `readStream` always uses SoapySDR's per-call `timeoutUs`, so this rarely matters here |
| `reconnect` | `1` (default) reconnects the stream automatically after a drop; `0` restores fail-fast behaviour |

```python
import SoapySDR
sdr = SoapySDR.Device("driver=aaronia,url=http://atc.local:54664,format=I16")
```

## Sample rates

`listSampleRates` reports the device's real ladder, each rung half the
one above it. `setSampleRate` snaps a request to the nearest rung and
logs when it has to.

**The rate is not the RF bandwidth.** Every sample reaches you, so a
waterfall spans the full rate, but only the middle 80% is flat and
calibrated — RTSA reports exactly 0.8 x Fs as the packet's frequency
range at every rate. Set the rate whose 80% covers the span you want to
look at: 15.36 MHz of sampling to see 12 MHz of spectrum. The edges of
the display are real data, just rolled off.

The ladder comes from the device. A backend that cannot be asked falls
back to the V6 ECO's: 61.44 MHz down to 120 kHz. A full V6 selects its
receiver clock and reaches higher, by how much is unsettled — see [the
note in HTTPSPEC](../docs/HTTPSPEC.md#unresolved-the-full-v6s-top-rate).
There, `getSampleRate` while streaming is the number to trust.

## What the probe reports

Over the HTTP backend, `SoapySDRUtil --probe` reports the attached
device rather than defaults: model, serial and firmware version, the
frequency and gain ranges with their steps, the sample-rate ladder,
clock sources and the RX antenna. A V6 ECO gives 5.5 MHz–8 GHz and
−55…+23 dBm.

Fields fall back independently, so a device that answers about frequency
but not gain still gets its frequency range published. The file and
native-SDK backends report the driver's defaults throughout.

`setClockSource` only reads: it accepts the device's current source and
warns for anything else. Change it in RTSA-Suite under Device > Stream
Clock Source.

## Streams

- **RX:** `CF32` (native) and `CS16`. The application-side stream format
  is a client-side conversion. Only the `format=` device argument
  changes what crosses the network.
- `readStream` honours `timeoutUs` and returns partial reads within the
  deadline, per the SoapySDR contract. Retuning while streaming is safe
  and needs no Aaronia licence.
- **`CS16` is a client-side conversion**, not a wire format — only the
  `format=` device argument changes network traffic. It quantises to
  int16, which costs precision on weak signals: lower the reference
  level, or stay on `CF32`.
- **TX** is `CF32`, single channel, and reported only on a native-SDK
  build opened by `serial=`. Over `url=` or `file=` the probe shows
  `0 Tx`. Bursts transmit immediately; timed TX (`SOAPY_SDR_HAS_TIME`)
  is not supported, and the whole path is hardware-unverified. The check
  is on the build and backend, not the hardware — an ECO has no
  transmitter but would still advertise TX if opened by serial.

## Time, gain, sensors

- **Stream timestamps.** Every RTSA packet carries a start time, so
  `readStream` flags every buffer `SOAPY_SDR_HAS_TIME` with epoch
  nanoseconds. Take them from there: `getHardwareTime("")` returns the
  most recent one but `0` before any packet has arrived, and SoapySDR
  has no way to say "no time yet".
- `hasHardwareTime("GPS")` reports a value rather than a capability —
  true only on the native-SDK backend with a valid fix.
  `getHardwareTime("GPS")` returns epoch nanoseconds.
- The single gain element, `REF`, is the Aaronia reference level in dBm.
  It is not an amplifier gain: raising it reduces sensitivity. Range and
  step come from the device — −55…+23 dBm in 0.5 dB steps on a V6 ECO.
- `readSensor("cumulative_drops")` counts the timestamp gaps the plugin has
  detected in the stream.

## Known limitations

- One RX channel through the plugin. `rx_channel=Rx2` selects the second
  antenna input; simultaneous dual-channel reads are available through
  the crate's Rust, Python and C APIs, not the SoapySDR interface.
- Enumeration advertises a default localhost candidate without probing
  it, because `find()` must not block on the network.
