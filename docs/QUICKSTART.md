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
        .span_frequency(15.36e6)    // sample rate (Fs), not RF bandwidth
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
cargo run --example http_iq_quickstart --features http -- 2440e6 15.36e6 http://localhost:54664
```

### Python

```bash
pip install python-aaronia
```

Check the server first. The command ships with the package and reports
what is wrong, and how to fix it:

```bash
aaronia-doctor http://localhost:54664
```

```python
import aaronia

with aaronia.open("http://localhost:54664", freq=2.44e9, bandwidth=10e6) as src:
    samples = src.read_samples_numpy(65536)   # numpy complex64
    print(samples[:4], src.cumulative_drops())
```

`bandwidth` picks a sample rate the hardware can run. Pass `rate=` to
name one exactly, or build an `AaroniaConfig` for the full set of
options.

### SoapySDR

Download the plugin archive for your platform from a
[release](https://github.com/isaacbentley/sdr-aaronia-rs/releases),
unpack it, and run the bundled installer:

```bash
./install.sh
SoapySDRUtil --probe="driver=aaronia,url=http://localhost:54664"
```

[APPS.md](APPS.md) covers GQRX, SDR++ and GNU Radio.
[../soapy-aaronia/README.md](../soapy-aaronia/README.md) covers
installing by hand and building from source.

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

**Span, sample rate and the "1 / 4" in the GUI.**
`span_frequency` is the IQ sample rate (Fs). The name comes from the
Aaronia API. Three numbers describe the same capture and they are all
different:

| Where you see it | Example | Meaning |
| --- | --- | --- |
| `span_frequency`, and `sampleFrequency` in packet metadata | 15.36 MHz | The sample rate, Fs |
| `startFrequency`..`endFrequency` in packet metadata | 12.288 MHz | Usable RF bandwidth: exactly 0.8 x Fs, at every rate |
| The Span control in the RTSA GUI | `1 / 4` | Decimation of the top rate, so Fs = 61.44 / 4 on an ECO |

**You get every sample, but only the middle 80% is calibrated.** An FFT
of the samples spans the full Fs. RTSA reports 0.8 x Fs as the packet's
frequency range, at every rate — a fixed rule, not a per-rate
measurement — and that is the part the anti-alias filter keeps flat and
the calibration covers. Data outside it still arrives, attenuated and
uncalibrated.

So **to see N Hz of spectrum, sample at N / 0.8**: 8 MHz of signal needs
10 MHz of sampling, and the lowest rung providing it is 15.36 MHz.
`iq_sample_rate_for_bandwidth` does that arithmetic;
`aaronia.sample_rate_for_bandwidth` is the Python equivalent.

The 80% figure holds up when measured. Averaging the receiver's own
noise floor on a V6 ECO, the response is flat to within 0.5 dB across
0.80 x Fs at 15.36 MHz sampling and 0.89 x Fs at 7.68 MHz — at or
beyond what is declared. At full span it is tighter: the analog filter
is about 1 dB down by the declared edge and 3 dB down at 0.84 x Fs,
which is why Aaronia's data sheet quotes 44 MHz of real-time bandwidth
for the ECO rather than the 49.152 MHz the device declares. Take the
declared span as the working figure and the data sheet as the
guaranteed one.

That sweep sees the antenna as well as the receiver, so the full-span
roll-off is an upper bound on how good the filter is, not a
measurement of it alone. It is enough to show the declared 80% is
physically grounded rather than an arbitrary number, which is what the
figure is used for here.

The device halves its top rate down a ten-rung ladder, shown in the GUI
as Full through `1 / 512`. On a V6 ECO that is 61.44 MHz down to
120 kHz, measured rung by rung. A full V6 has a selectable receiver
clock and starts higher.

**Pass one of those rates and you get it exactly.** Anything else is
silently adjusted, and not to the nearest rate: the server reads the
requested span as the *usable bandwidth* you want and picks the rate
whose usable span is closest. Asking for 2.5 MHz gives Fs = 3.84 MHz,
whose usable span is 3.07 MHz, rather than the numerically closer
1.92 MHz. Verified across nine requests on a V6 ECO.

`AaroniaSource::get_source_info()` reports the rate the server is
actually sending once packets are flowing, so read it back rather than
assuming.

The crate rejects a rate that would violate the IQ-mode constraint
before streaming starts, but it does not second-guess the ladder.

**Retuning has no effect.**
The `/control` endpoint applies a frequency change only when
`frequencyCenter` and `frequencySpan` are both present. A request
carrying one of them returns `{"success":true}` and is ignored. This
crate always sends the complete tuple, so use `set_center_frequency`
instead of issuing PUTs directly. No Aaronia licence is involved: the
crate tunes through `/control`, which needs none.

**Network saturation at high sample rates.**
Float32 needs roughly 490 MB/s at the 61.44 MS/s top rate. On anything other than
localhost, use the `I16` wire format via
`AaroniaConfig::low_bandwidth_mode()` or the `format=I16` SoapySDR
device argument to halve that. This changes the wire format itself,
unlike a client-side `CS16` conversion.

**Dropped samples.**
`cumulative_drops()` counts the timestamp gaps the client has detected
in the stream (one per gap, however many samples it spanned), and
`take_overrun()` flags the read that follows one.
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
    .span_frequency(15.36e6);
let mut source = AaroniaSource::new(config).await?;
# Ok(())
# }
```

Build with `--features native-sdk`. Aaronia does not ship a macOS SDK,
so this path is unavailable there. The native-SDK paths are
hardware-unverified in this project; see the verification table in
[VERIFICATION.md](VERIFICATION.md).
