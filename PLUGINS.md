# Aaronia Spectran V6 SDR Plugins

`sdr-aaronia-rs` provides two powerful ways to integrate Aaronia Spectran V6 devices into your existing SDR workflows: **Seify** and **SoapySDR**.

## 1. Seify Plugin (Rust Native)

[Seify](https://github.com/mryndzionek/seify) is a modern, Rust-native SDR hardware abstraction layer. We provide a backend for `seify` directly within this crate.

### Usage

To use the Seify plugin, enable the `seify` feature in your `Cargo.toml`:

```toml
[dependencies]
sdr-aaronia-rs = { version = "0.3.5", features = ["seify"] }
```

In your code, you can instantiate the device using the `AaroniaSeifyDevice` backend directly, or by passing the appropriate arguments string if it's registered with the global registry.

```rust
use sdr_aaronia_rs::seify_impl::AaroniaSeifyDevice;
use seify::{Args, RxDevice, RxStreamer, DeviceInfo};
use seify::dev::DynDeviceBackend;

// Initialize with the HTTP endpoint URL
let mut args = Args::new();
args.set("url", "http://localhost:54664");

// Open the device
let dev = AaroniaSeifyDevice::from_args(&args).expect("Failed to open Aaronia device");

// Start streaming (CF32 complex floats)
let rx = dev.rx_device().expect("Failed to get RX device");
let mut streamer = rx.rx_streamer(&[0], Args::new()).expect("Failed to create RX streamer");
streamer.activate_at(None).expect("Failed to activate stream");

let mut buffer = [num_complex::Complex32::new(0.0, 0.0); 1024];
let read = streamer.read(&mut [&mut buffer], 1_000_000).expect("Read failed");
println!("Read {} samples", read);
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

The driver expects the `url` argument to connect to the Spectran V6 RTSA HTTP server.

```bash
# Example testing with SoapySDRUtil
SoapySDRUtil --probe="driver=aaronia,url=http://localhost:54664"
```

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

### Bandwidth Trade-off: CS16 vs CF32

The Aaronia hardware natively supplies 16-bit signed integer (`CS16`) I/Q data.
- **`CS16` (Default)**: The plugin's native stream format is `CS16`. This requires exactly half the memory and bandwidth (32 bits per complex sample) compared to 32-bit floats. **This is highly recommended for network streaming** as it avoids overhead and allows for higher sample rates.
- **`CF32`**: The plugin also supports `CF32`. When requesting this format, the C++ driver automatically converts the 16-bit integers to 32-bit floats on the fly. This format is convenient for DSP tools like GNU Radio that prefer floats, but it doubles the required memory bandwidth for sample transfers.

## 3. Metrics and Error Handling

Both the native Rust API and the Python/C++ bindings expose critical metrics to monitor the health of your RTSA stream:

- **Hardware Timestamps**: You can retrieve the precise hardware timestamp (in nanoseconds) of the last received block using `last_timestamp_ns`.
- **Overruns**: The `take_overrun()` function checks if the internal buffer has overflown since the last check, allowing you to react to drops on the client side.
- **Cumulative Drops**: The `cumulative_drops()` function reports the total number of blocks dropped by the Aaronia server itself due to network backpressure.

In SoapySDR, you can access these metrics via the `readSensor()` API:
```python
# Check for server-side drops
drops = sdr.readSensor("cumulative_drops")
print(f"Dropped blocks: {drops}")
```
