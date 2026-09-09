# Aaronia Spectran V6 SDR Plugins

`sdr-aaronia-rs` provides two powerful ways to integrate Aaronia Spectran V6 devices into your existing SDR workflows: **Seify** and **SoapySDR**.

## 1. Seify Plugin (Rust Native)

[Seify](https://github.com/FutureSDR/seify) is a Rust-native SDR hardware abstraction layer. We provide a backend for `seify` directly within this crate.

### Usage

To use the Seify plugin, enable the `seify` feature in your `Cargo.toml`:

```toml
[dependencies]
sdr-aaronia-rs = { version = "0.10", features = ["seify"] }
```

Instantiate the device with `SpectranSeifyDevice::from_args` and use it directly (or via `seify::dev::DynDeviceBackend`). The backend is **not** part of seify's built-in enumeration registry — `seify::enumerate()` will not discover it.

`url=` selects the HTTP backend and `file=` playback. `sdk=true` (or
`serial=<device serial>`) selects the Aaronia native SDK; that needs the
crate built with both features — `features = ["seify", "native-sdk"]` —
on Windows or Linux with RTSA-Suite PRO installed. Without the feature
the request is a clean error, never a fallback to HTTP.

`SpectranSeifyDevice` owns a tokio runtime. Drop it from synchronous
code: dropping it inside an `async` context (a `#[tokio::test]`, a task)
is a tokio panic, "Cannot drop a runtime in a context where blocking is
not allowed".

```rust,no_run
# #[cfg(feature = "seify")]
# fn demo() {
use sdr_aaronia_rs::seify_impl::SpectranSeifyDevice;
use seify::{Args, RxDevice, RxStreamer, DeviceInfo};
use seify::dev::DynDeviceBackend;

// Initialize with the HTTP endpoint URL
let mut args = Args::new();
args.set("url", "http://localhost:54664");

// Open the device
let dev = SpectranSeifyDevice::from_args(&args).expect("Failed to open Aaronia device");

// Start streaming (CF32 complex floats)
let rx = dev.rx_device().expect("Failed to get RX device");
let mut streamer = rx.rx_streamer(&[0], Args::new()).expect("Failed to create RX streamer");
streamer.activate_at(None).expect("Failed to activate stream");

let mut buffer = [num_complex::Complex32::new(0.0, 0.0); 1024];
let read = streamer.read(&mut [&mut buffer], 1_000_000).expect("Read failed");
println!("Read {} samples", read);
# }
# fn main() {}
```

> **Note on Bandwidth:** Seify's `RxStreamer` trait natively expects `Complex32` (CF32) buffers, so data will be transferred as 32-bit floats.

---

## 2. SoapySDR Plugin (C++)

[SoapySDR](https://github.com/pothosware/SoapySDR) is a popular C++ API and runtime library for interfacing with SDR devices. We provide a C++ module in the `soapy-aaronia/` directory that bridges SoapySDR to the `sdr-aaronia-rs` C ABI (FFI).

### Building the SoapySDR Plugin

The plugin requires `cmake`, `SoapySDR`, and the compiled `sdr-aaronia-rs` static/dynamic library. 

1. Ensure the Rust crate is built with the `ffi` feature (which is default).
   ```bash
   cargo build --release
   ```

2. Build the CMake project:
   ```bash
   cd soapy-aaronia
   mkdir build && cd build
   cmake ..
   make
   ```

   On Windows, point CMake at the SoapySDR you will load the module into
   — `-DSoapySDR_DIR="C:\Program Files\PothosSDR\cmake"` for PothosSDR,
   or `<radioconda>\Library\cmake` for a GNU Radio (radioconda) install —
   and build with **MSVC**. Both of those runtimes are MSVC-built, and a
   SoapySDR module is C++ (virtual classes, `std::string` across the
   boundary), so a MinGW-built module cannot load into them; the Rust
   side must then be the `x86_64-pc-windows-msvc` toolchain too. Add
   `-DAARONIA_NATIVE_SDK=ON` for the native-SDK backend.

3. Ensure SoapySDR can find the plugin. You can install it to your system's Soapy modules directory (e.g. `/usr/local/lib/SoapySDR/modules0.8/`) or set the `SOAPY_SDR_PLUGIN_PATH` environment variable:
   ```bash
   export SOAPY_SDR_PLUGIN_PATH=$(pwd)/soapy-aaronia/build
   ```

### Verifying the Plugin

Check that SoapySDR discovers the `aaronia` driver:
```bash
SoapySDRUtil --info
# Should show:
# Available factories... aaronia
```

### Usage

`url=` connects to an RTSA-Suite HTTP server block, `file=` plays back a
recording, and `sdk=true` (or `serial=<device serial>`) opens the device
through the Aaronia native SDK — a build with `-DAARONIA_NATIVE_SDK=ON`
on a machine with RTSA-Suite PRO installed. A bare `driver=aaronia` keeps
its old meaning, the HTTP server on localhost; when the SDK is installed,
`SoapySDRUtil --find` lists a second, `sdk=true` entry beside it.

```bash
# Example testing with SoapySDRUtil
SoapySDRUtil --probe="driver=aaronia,url=http://localhost:54664"
SoapySDRUtil --probe="driver=aaronia,sdk=true"
```

Two things learned running the plugin on Windows inside a GNU Radio
(radioconda) install: that build's Python binding rejects the `dict`
form for every driver — `SoapySDR.Device("driver=aaronia,sdk=true")`
works where `SoapySDR.Device(dict(driver="aaronia", sdk="true"))` raises
"no match" — and a host process that has already loaded its own Qt6
(GNU Radio Companion itself) cannot also load the Aaronia SDK, which
brings a different Qt6; use `url=` from inside GRC. Loading the SDK from
a host whose *directory* merely contains Qt6 copies, such as
`SoapySDRUtil` in radioconda's `Library\bin`, works since 0.8.3.

In Python (using `SoapySDR` python bindings):
```python
import SoapySDR

# Open the device
args = dict(driver="aaronia", url="http://localhost:54664")
sdr = SoapySDR.Device(args)

# Configure stream
sdr.setSampleRate(SoapySDR.SOAPY_SDR_RX, 0, 1e6)
sdr.setFrequency(SoapySDR.SOAPY_SDR_RX, 0, 100e6)

# Setup stream
rxStream = sdr.setupStream(SoapySDR.SOAPY_SDR_RX, SoapySDR.SOAPY_SDR_CS16)
sdr.activateStream(rxStream)
# ... read samples ...
```

### Bandwidth Trade-off: wire format vs stream format

Two different knobs, easy to confuse:

- **App-side stream format** (`setupStream(..., CS16)` / `CF32`): what
  the plugin hands your application. The C ABI always transfers CF32;
  requesting `CS16` adds a client-side float→int16 conversion. It saves
  application memory bandwidth but **zero network traffic**. The
  plugin's native format is `CF32`.
- **Network wire format** (device arg `format=I16`, optionally
  `scale=N`): tells the RTSA HTTP server to send int16 on the wire —
  this is the genuine low-bandwidth mode, halving network traffic:

```python
sdr = SoapySDR.Device("driver=aaronia,url=http://atc.local:54664,format=I16")
```

## 3. Transmit

The plugin reports a TX channel only when the source it opened is the
native-SDK backend — a build configured with `-DAARONIA_NATIVE_SDK=ON`
on Windows or Linux, with the Aaronia SDK installed. A device opened
over `url=` or `file=` has no transmit path and the probe shows `0 Tx`;
`setupStream(TX)` is never reachable there. Bursts are sent for immediate
transmission; timed TX is not supported. The whole TX path is
hardware-unverified. See [docs/VERIFICATION.md](docs/VERIFICATION.md).

## 4. Metrics and Error Handling

Both the native Rust API and the Python/C++ bindings expose critical metrics to monitor the health of your RTSA stream:

- **Hardware Timestamps**: You can retrieve the precise hardware timestamp (in nanoseconds) of the last received block using `last_timestamp_ns`.
- **Overruns**: The `take_overrun()` function checks if the internal buffer has overflown since the last check, allowing you to react to drops on the client side.
- **Cumulative Drops**: `cumulative_drops()` counts the timestamp gaps the client's drop detector has seen in the stream. Each is one gap event, however many samples it spanned.

In SoapySDR, you can access these metrics via the `readSensor()` API:
```python
# Check for gaps in the stream
drops = sdr.readSensor("cumulative_drops")
print(f"Gaps seen: {drops}")
```

### Sensors

Over the **HTTP backend** the plugin also surfaces the device's live
telemetry from `/healthstatus` as SoapySDR sensors. `listSensors()`
returns only those the device is currently reporting:

```python
for name in sdr.listSensors():
    info = sdr.getSensorInfo(name)
    print(f"{name} = {sdr.readSensor(name)} {info.units}")
# fpga_temp = 54.3 C
# frontend_temp = 65.1 C
# adc_range = 29.4 dB        # headroom below full scale; near 0 is close to clipping
# usb_buffer = 0.0625        # transfer-buffer fill, fraction 0-1
# dsp_buffer = 0
# gps_satellites = 0
# ...plus cumulative_drops
```

`adc_range` is the one to watch when setting the reference level, and a
climbing `usb_buffer` is the first sign the host is not draining the
stream fast enough. The reads are briefly cached, so a probe's burst of
`readSensor` calls costs one HTTP fetch, and they go through a separate
connection from the sample stream — polling sensors during a capture does
not stall sample delivery. Over the **native SDK** backend only
`cumulative_drops` is reported: the raw SDK's own `AARTSAAPI_ConfigHealth`
tree reads all zeros (the live telemetry is populated by RTSA-Suite, the
managing application, not by the raw SDK), so exposing it there would
report a device permanently at 0 °C.

### Bandwidth

SoapySDR keeps sample rate and analog/usable bandwidth as separate knobs.
The RTSA's alias-free bandwidth is ~0.8x its sample rate, exposed here:

```python
sdr.setBandwidth(SoapySDR.SOAPY_SDR_RX, 0, 10e6)   # asks for ~10 MHz usable
print(sdr.getBandwidth(SoapySDR.SOAPY_SDR_RX, 0))  # the bandwidth actually delivered
print(sdr.listBandwidths(SoapySDR.SOAPY_SDR_RX, 0))
```

`setBandwidth` maps the request to the nearest sample-rate rung and
drives `setSampleRate`; the two stay consistent, so setting either
updates the other.

### Clock source

`device/sclksource` selects what disciplines the receiver's clock. A V6 ECO
offers `Consumer`, `Oscillator`, `GPS`, `PPS`, `10MHz` and three `... Provider`
variants; `listClockSources` reports whatever the device actually offers.

```python
print(sdr.listClockSources())   # ['Consumer', 'Oscillator', 'GPS', 'PPS', '10MHz', ...]
sdr.setClockSource("10MHz")     # lock to a house 10 MHz reference
print(sdr.getClockSource())
```

`setClockSource` now writes the device; it previously only reported the current
source and told you to change it in RTSA-Suite. Over HTTP the write is read back
to confirm, because a `/remoteconfig` PUT naming a block that is not in the
running mission answers 200 and changes nothing.

If the device does not take the source — usually because it does not offer it —
the plugin logs a `SOAPY_SDR_ERROR` naming the sources it *does* offer, and does
not cache the requested value. `getClockSource()` therefore keeps reporting what
the device is actually running on, never what you asked for.

**Why this matters for multi-device work.** Point several receivers at one
10 MHz / PPS / GPS reference and their per-packet hardware timestamps share a
timebase, so captures can be correlated afterwards. There is no commanded
synchronous start: the SDK exposes no set-time and no arm-at-time call, so you
align on timestamps in post rather than arming devices at an instant.

