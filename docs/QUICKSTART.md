# Quickstart

This crate reads data from an RTSA-Suite HTTP server. That server is
configured in Aaronia's application, not here. Most first-time setup
problems occur at that stage, so it is covered first.

For the native SDK (Windows and Linux only), skip to
[Native SDK](#native-sdk). No mission configuration is required.

## 1. Configure an HTTP Server block

In RTSA-Suite PRO:

1. Add your device block (for example **SPECTRAN V6**) to the mission
   and start it. A live spectrum should appear.
2. Set the device to **IQ mode**. `AaroniaSource` reads IQ data. Spectra
   streams are available through the lower-level `HttpEndpointsClient`,
   but the sample APIs expect IQ.
3. Add an **HTTP Server** block.
4. Connect the device block's output to the HTTP Server block's input.
   Without this connection the server still starts and answers `/info`,
   but `/stream` returns no data.
5. Confirm the port. The default is **54664**, which this crate assumes.

Save the mission so the configuration survives a restart.

## 2. Verify from the command line

Check that the server is reachable and streaming before writing code.
Run this from the machine that will run your program:

```bash
curl http://localhost:54664/info
```

The response is JSON containing `name`, `uuid`, `port` and `mission`.
Next confirm that an input exists and carries IQ data:

```bash
curl http://localhost:54664/inputs
curl http://localhost:54664/sample
```

When the mission is configured correctly, `/sample` reports
`"payload":"iq"` and a non-zero `samples` count. A `spectra` payload
means the device is not in IQ mode; return to step 2.

For a server on another machine, substitute its hostname in every URL
below (for example `http://spectran.local:54664`) and allow inbound
traffic on port 54664 through the RTSA host's firewall.

## 3. Read samples

### Rust

```rust,no_run
use sdr_aaronia_rs::{AaroniaConfig, AaroniaSource};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = AaroniaConfig::from_http("http://localhost:54664")
        .center_frequency(2.44e9)
        .span_frequency(12.288e6)   // sample rate, not RF bandwidth
        .reference_level(-20.0);

    let mut source = AaroniaSource::new(config).await?;
    source.start_streaming().await?;

    let mut buffer = Vec::new();
    let n = source.read_samples(&mut buffer, 65_536).await?;
    println!("read {n} samples, first = {:?}", buffer.first());

    source.stop_streaming().await?;
    Ok(())
}
```

```bash
# args: <center-hz> <sample-rate-hz> <url>
cargo run --example http_iq_quickstart --features http -- 2440e6 12.288e6 http://localhost:54664
```

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
print(samples[:4], src.cumulative_drops())
src.stop_streaming()
```

### SoapySDR

```bash
SoapySDRUtil --probe="driver=aaronia,url=http://localhost:54664"
```

[APPS.md](APPS.md) covers GQRX, SDR++ and GNU Radio.
[../soapy-aaronia/README.md](../soapy-aaronia/README.md) covers
installing the plugin.

## 4. Troubleshooting

**"Failed to connect … Is it running and accessible?"**
The mission is not running, the HTTP Server block is absent, or the port
differs. Check `curl /info` first. Connection attempts retry transient
failures for several seconds, so a persistent error indicates a real
fault.

**Connects, but reads time out.**
Usually the missing connection from step 4: the device block's output is
not wired to the HTTP Server block. `curl /sample` shows whether data is
present.

**Span and sample rate.**
`span_frequency` is the IQ sample rate (Fs). The name comes from the
Aaronia API. It is not the usable RF bandwidth, which is smaller because
of the anti-alias filter (roughly 49 MHz usable within a 61.44 MHz Fs
capture). The device derives its decimation steps from a 61.44 MHz
clock. Over HTTP the RTSA-Suite accepts rates across its supported
range, and the crate rejects a rate that would violate the IQ-mode
constraint before streaming starts.

**Retuning has no effect.**
The `/control` endpoint applies a frequency change only when
`frequencyCenter` and `frequencySpan` are both present. A request
carrying one of them returns `{"success":true}` and is ignored. This
crate always sends the complete tuple, so use `set_center_frequency`
instead of issuing PUTs directly. No Aaronia licence is involved; the
licence gates `/remoteconfig`, which the crate does not use for tuning.

**Network saturation at high sample rates.**
Float32 requires roughly 740 MB/s at 92 MSPS. On anything other than
localhost, use the `I16` wire format via
`AaroniaConfig::low_bandwidth_mode()` or the `format=I16` SoapySDR
device argument to halve that. This changes the wire format itself,
unlike a client-side `CS16` conversion.

**Dropped samples.**
`cumulative_drops()` and `take_overrun()` report server-side gaps.
Steady growth means the link or the consumer cannot keep pace. Reduce
the sample rate, switch to `I16`, or read in larger blocks.

## Native SDK

On Windows or Linux with the Aaronia SDK installed, no mission
configuration is required:

```rust,no_run
use sdr_aaronia_rs::{AaroniaConfig, AaroniaSource};

# async fn run() -> anyhow::Result<()> {
let config = AaroniaConfig::default()
    .force_native_sdk()
    .center_frequency(2.44e9)
    .span_frequency(12.288e6);
let mut source = AaroniaSource::new(config).await?;
# Ok(())
# }
```

Build with `--features native-sdk`. Aaronia does not ship a macOS SDK,
so this path is unavailable there. The native-SDK paths are
hardware-unverified in this project; see the verification table in the
[README](../README.md).
