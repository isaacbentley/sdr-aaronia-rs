# python-aaronia

Python bindings for
[`sdr-aaronia-rs`](https://github.com/isaacbentley/sdr-aaronia-rs).
Stream IQ samples from Aaronia SPECTRAN V6 devices, through an
RTSA-Suite PRO HTTP server block or the native SDK, or play back
recorded `.rtsa` files, into NumPy or Apache Arrow.

- **PyPI package:** `python-aaronia` · **importable module:** `aaronia`
- **Wheels:** abi3, CPython ≥ 3.9, one wheel per OS and architecture,
  plus an sdist for other platforms. Building from the sdist requires a
  Rust toolchain.
- **License:** GPL-3.0-or-later

## Install

```bash
pip install python-aaronia
```

From a checkout, which requires Rust and [maturin](https://maturin.rs):

```bash
cd python-aaronia
maturin develop --release
```

To reach the Aaronia native SDK as well — a Spectran on this machine's
USB, with RTSA-Suite PRO installed, on Windows or Linux — build with the
feature; the default wheel speaks only HTTP and files:

```bash
maturin develop --release --features native-sdk
```

Check your setup before writing any code:

```bash
aaronia-doctor http://localhost:54664
```

It reports whether the server is reachable, whether the mission has an
input carrying IQ, and what rate the device is running, and names the
fix for each failure.

## Quickstart

```python
import aaronia

with aaronia.open(
    "http://localhost:54664", center_frequency_hz=2.44e9, bandwidth_hz=10e6
) as src:
    for block in src.blocks(65536):           # numpy complex64 arrays
        process(block)
```

`aaronia.open()` connects and starts streaming in one call. `bandwidth_hz`
asks for that much usable spectrum and picks a sample rate the hardware
can actually run; pass `sample_rate_hz=` instead to name one exactly.
Use
`file="capture.rtsa"` in place of the URL to play back a recording.

Iterating with `blocks()` ends when the stream closes. To read on your
own schedule, or for Apache Arrow:

```python
src = aaronia.open(center_frequency_hz=2.44e9, sample_rate_hz=15.36e6, format="I16")
```

`sdk=True` opens the device through the native SDK instead of a server
(`serial=` picks one of several). It is an error, not a fallback, when
the SDK is missing — a capture never quietly comes from another backend:

```python
src = aaronia.open(sdk=True, center_frequency_hz=2.44e9, sample_rate_hz=15.36e6)
src = aaronia.open(sdk=True, serial="C2-P-03000105", center_frequency_hz=2.44e9)
samples = src.read_samples_numpy(65536)       # numpy complex64 array
batch = src.read_samples_arrow(65536)         # pyarrow FixedSizeListArray of [re, im]
src.set_center_frequency_hz(2.41e9)              # live retune, no teardown
print(src.cumulative_drops(), src.take_overrun(), src.last_timestamp_ns())
src.stop_streaming()
```

For full control, build a `SpectranConfig` and pass it to
`SpectranSource.start_streaming()`; `open()` is a shorthand for the
common fields.

The
[quickstart](https://github.com/isaacbentley/sdr-aaronia-rs/blob/main/docs/QUICKSTART.md)
covers configuring the RTSA-Suite HTTP Server block for HTTP sources. Native SDK sources do not require that server.

## Sample rate and RF span

The module-level `sample_rates()` and `sample_rate_for_bandwidth()` functions
remain legacy HTTP/ECO helpers. They are not capabilities of every native device.
For a native IQ receiver, request RF span separately and inspect packet geometry:

```python
cfg = aaronia.SpectranConfig()
cfg.force_native_sdk = True
cfg.device_serial = "YOUR_DEVICE_SERIAL"
cfg.native_mode = "iqreceiver"
cfg.rf_span_hz = 44e6
cfg.center_frequency_hz = 280e6
cfg.reference_level_dbm = -50
src = aaronia.SpectranSource()
src.start_streaming(cfg)
print(src.capabilities())                 # schema_version + capabilities
samples = src.read_samples_numpy(65536)
print(src.source_info())                  # observed rate and RF span
src.set_rf_span_hz(20e6)
src.stop_streaming()
```

IQ sample clock and valid RF span are distinct. The tested ECO delivered 44 MHz
RF span at about 59.214 MS/s, and 20 MHz at 30 MS/s. Native configuration bounds
may exceed the model's rated bandwidth. Neither those bounds nor RF span certifies
amplitude calibration. Unknown metadata stays unknown. See the
[qualification matrix](../docs/NATIVE_SDK_COMPATIBILITY.md).

For HTTP only, the legacy helper uses the measured 0.8×Fs filter width and ECO
rate ladder. Prefer `source_info()` for what the stream actually delivered.

## Choosing a wire format

`format` decides what crosses the network, and it matters more than it
looks. Measured against a live server at 15.36 MS/s over a LAN:

| format | bytes/sample | delivered | drops |
| --- | --- | --- | --- |
| `F32` (default) | 8 | 6.5 MS/s | 290 |
| `F16` | 4 | 15.1 MS/s | 9 |
| `I16` | 4 | 15.1 MS/s | 12 |

`F32` needs 123 MB/s at that rate and the link could not carry it, so
most of the capture was dropped. Either half-width format fits.

`I16` has one trap: the server sends `round(value * scale)`, so the
quantisation step is `1 / scale`, and the default of 16384 gives a step
of 6.1e-5. A quiet band's noise floor is smaller than that — on the
same server, **68% of `I16` samples came back exactly zero** while
`F32` had none. Pass `scale=`, or lower `reference_level_dbm` for more
gain:

```python
aaronia.open(url, center_frequency_hz=2.44e9, sample_rate_hz=15.36e6, format="I16", scale=1e6)
```

At `scale=1e6` the zero fraction measured 0.0% and the amplitude
matched `F32`. `F16` needs no such tuning, which makes it the simpler
choice when the link is the constraint.

## Configuration (`SpectranConfig`)

Every field is readable and writable.

| Field | Meaning |
| --- | --- |
| `http_base_url` | RTSA-Suite HTTP server URL; pins the HTTP backend |
| `file_path` | Path to a recorded `.rtsa` file; pins the file backend |
| `device_serial` | Device selection for the native-SDK backend |
| `force_native_sdk` | `True` pins the source to the native SDK; missing SDK is an error |
| `native_family` | Optional bare SDK family, e.g. `"spectranv6eco"` |
| `native_mode` | Optional receive-only `"iqreceiver"` or `"raw"` |
| `rf_span_hz` | Optional requested native IQ receiver RF span, Hz |
| `receiver_clock` | Optional enabled raw-mode SDK clock label |
| `decimation_factor` | Optional raw-mode power-of-two factor |
| `center_frequency_hz` | Center frequency, Hz |
| `sample_rate_hz` | IQ sample rate (Fs), Hz |
| `reference_level_dbm` | Reference level, dBm |
| `format` | HTTP wire format: `"F32"`, `"F16"` or `"I16"`. `I16` is the low-bandwidth network mode |
| `scale` | Integer encode multiplier for `I16` (see below). None uses the server default |
| `receiver_channel` | `"Rx1"` (default), `"Rx2"`, or `"Rx1And2"` (native SDK, full V6) |
| `read_timeout_s` | Seconds a blocking read waits before `SpectranTimeoutError` (default `30.0`) |
| `auto_reconnect` | Reconnect the HTTP stream after a drop (default `True`) |

Unknown `format`/`receiver_channel` strings raise `ValueError` instead of
silently defaulting.

## Behaviour

- **One copy per read.** Samples are copied once from the Rust receive
  buffer into a NumPy or Arrow owned buffer, which is then safe to hold
  indefinitely. This is not zero-copy; one copy is the accurate count.
- **Blocking calls release the GIL.** Other Python threads keep running;
  `KeyboardInterrupt` is delivered between calls. Reads block until
  `count` samples arrive or `cfg.read_timeout_s` seconds (default 30)
  elapse, which raises `SpectranTimeoutError`.
- **Connecting retries transient failures**, up to 4 attempts within a
  10 second budget, so a cold `*.local` hostname or a server that is
  still starting does not fail on the first attempt.
- **Dropped streams reconnect automatically** when `auto_reconnect` is
  enabled, which is the default. The reader reopens the stream,
  re-applies the current tuning, and flags the first read after the gap
  through `take_overrun()`. After five failed attempts the stream ends
  and reads raise `SpectranStreamClosed`.
- **Typed exceptions.** `SpectranConnectionError` (unreachable endpoint),
  `SpectranTimeoutError`, `SpectranHardwareError` (device and SDK errors)
  and `ValueError` (invalid configuration), mapped from the Rust error
  enum with the full cause chain in the message.
  `SpectranStreamClosed` subclasses `SpectranConnectionError` and means
  the stream finished rather than failed; `blocks()` ends on it, while
  a timeout or transport failure still raises.
- **Dual-channel** reads (`receiver_channel = "Rx1And2"` with
  `read_samples_dual_numpy(count)`, returning two time-aligned arrays)
  require the native-SDK backend: Windows or Linux with the Aaronia SDK
  installed, and a two-input V6. This path is hardware-unverified; the
  development device is a single-channel V6 ECO.

## Source methods

| Method | Purpose |
| --- | --- |
| `start_streaming(cfg)` / `stop_streaming()` | Session lifecycle |
| `with src: ...` | Stops streaming on the way out, including after an exception |
| `blocks(count)` | Iterate `count`-sample arrays until the stream closes |
| `read_samples_numpy(count)` | NumPy `complex64` array |
| `read_samples_arrow(count)` | PyArrow `FixedSizeListArray` of `[re, im]` float32 pairs |
| `read_samples_dual_numpy(count)` | `(rx1, rx2)` NumPy arrays (dual-channel captures) |
| `set_center_frequency_hz(hz)` / `set_sample_rate_hz(hz)` / `set_reference_level_dbm(dbm)` | Live retuning |
| `cumulative_drops()` | Timestamp gaps detected in the stream so far (gap events, not samples) |
| `take_overrun()` | True once per detected receive-side overrun |
| `last_timestamp_ns()` | Epoch-ns timestamp of the last received block (HTTP backend; 0 otherwise) |

## Module functions

| Function | Purpose |
| --- | --- |
| `open(url=None, *, center_frequency_hz, sample_rate_hz, bandwidth_hz, reference_level_dbm, file, format, scale, read_timeout_s)` | Configure, connect and start streaming in one call |
| `sample_rates()` | The V6 ECO's sample rates, highest first (see [Sample rates](#sample-rates)) |
| `sample_rate_for_bandwidth(hz)` | Lowest rate covering that much spectrum |
| `diagnose(url)` | `(ok, message, fix)` for each setup check; what `aaronia-doctor` prints |
