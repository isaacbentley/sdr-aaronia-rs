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

with aaronia.open("http://localhost:54664", freq=2.44e9, bandwidth=10e6) as src:
    for block in src.blocks(65536):           # numpy complex64 arrays
        process(block)
```

`aaronia.open()` connects and starts streaming in one call. `bandwidth`
asks for that much usable spectrum and picks a sample rate the hardware
can actually run; pass `rate=` instead to name one exactly. Use
`file="capture.rtsa"` in place of the URL to play back a recording.

Iterating with `blocks()` ends when the stream closes. To read on your
own schedule, or for Apache Arrow:

```python
src = aaronia.open(freq=2.44e9, rate=15.36e6, format="I16")
samples = src.read_samples_numpy(65536)       # numpy complex64 array
batch = src.read_samples_arrow(65536)         # pyarrow FixedSizeListArray of [re, im]
src.set_center_frequency(2.41e9)              # live retune, no teardown
print(src.cumulative_drops(), src.take_overrun(), src.last_timestamp_ns())
src.stop_streaming()
```

For full control, build an `AaroniaConfig` and pass it to
`AaroniaSource.start_streaming()`; `open()` is a shorthand for the
common fields.

The
[quickstart](https://github.com/isaacbentley/sdr-aaronia-rs/blob/main/docs/QUICKSTART.md)
covers configuring the RTSA-Suite HTTP Server block, which everything
above depends on.

## Sample rates

The device runs a ladder of rates rather than a continuous range: each
rung is half the one above it. Ask for anything else and it quietly
uses the nearest rung, leaving your program computing against a rate
that is not in use.

```python
aaronia.sample_rates()                  # every rate, highest first
aaronia.sample_rate_for_bandwidth(8e6)  # 15.36e6: the lowest rate covering 8 MHz
```

A rate carries only 80% of itself as alias-free bandwidth — measured
at four rates on a V6 ECO — which is why 8 MHz of spectrum needs
15.36 MHz of sampling.

`sample_rates()` returns the ladder for a SPECTRAN V6 ECO — 61.44 MHz
down to 120 kHz — which is measured, rung by rung. A full V6 has a
selectable receiver clock and can go higher, and exactly how much
higher is not settled; see
[the note in HTTPSPEC](https://github.com/isaacbentley/sdr-aaronia-rs/blob/main/docs/HTTPSPEC.md#unresolved-the-full-v6s-top-rate).
On that hardware, take the rate the device reports over the computed
ladder: it arrives in the stream metadata, and `diagnose()` prints it.

## Configuration (`AaroniaConfig`)

Every field is readable and writable.

| Field | Meaning |
| --- | --- |
| `http_base_url` | RTSA-Suite HTTP server URL; pins the HTTP backend |
| `file_path` | Path to a recorded `.rtsa` file; pins the file backend |
| `device_serial` | Device selection for the native-SDK backend |
| `center_freq` | Center frequency, Hz |
| `sample_rate` | IQ sample rate, Hz (the Aaronia "span") |
| `reference_level` | Reference level, dBm |
| `format` | HTTP wire format: `"F32"`, `"F16"` or `"I16"`. `I16` is the low-bandwidth network mode |
| `receiver_channel` | `"Rx1"` (default), `"Rx2"`, or `"Rx1And2"` (native SDK, full V6) |
| `read_timeout` | Seconds a blocking read waits before `AaroniaTimeoutError` (default `30.0`) |
| `auto_reconnect` | Reconnect the HTTP stream after a drop (default `True`) |

Unknown `format`/`receiver_channel` strings raise `ValueError` instead of
silently defaulting.

## Behaviour

- **One copy per read.** Samples are copied once from the Rust receive
  buffer into a NumPy or Arrow owned buffer, which is then safe to hold
  indefinitely. This is not zero-copy; one copy is the accurate count.
- **Blocking calls release the GIL.** Other Python threads keep running;
  `KeyboardInterrupt` is delivered between calls. Reads block until
  `count` samples arrive or `cfg.read_timeout` seconds (default 30)
  elapse, which raises `AaroniaTimeoutError`.
- **Connecting retries transient failures**, up to 4 attempts within a
  10 second budget, so a cold `*.local` hostname or a server that is
  still starting does not fail on the first attempt.
- **Dropped streams reconnect automatically** when `auto_reconnect` is
  enabled, which is the default. The reader reopens the stream,
  re-applies the current tuning, and flags the first read after the gap
  through `take_overrun()`. After five failed attempts the stream ends
  and reads raise `AaroniaStreamClosed`.
- **Typed exceptions.** `AaroniaConnectionError` (unreachable endpoint),
  `AaroniaTimeoutError`, `AaroniaHardwareError` (device and SDK errors)
  and `ValueError` (invalid configuration), mapped from the Rust error
  enum with the full cause chain in the message.
  `AaroniaStreamClosed` subclasses `AaroniaConnectionError` and means
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
| `set_center_frequency(hz)` / `set_sample_rate(hz)` / `set_reference_level(dbm)` | Live retuning |
| `cumulative_drops()` | Total server-reported dropped samples |
| `take_overrun()` | True once per detected receive-side overrun |
| `last_timestamp_ns()` | Epoch-ns timestamp of the last received block (HTTP backend; 0 otherwise) |

## Module functions

| Function | Purpose |
| --- | --- |
| `open(url=None, *, freq, rate, bandwidth, ref_level, file, format, read_timeout)` | Configure, connect and start streaming in one call |
| `sample_rates()` | The V6 ECO's sample rates, highest first (see [Sample rates](#sample-rates)) |
| `sample_rate_for_bandwidth(hz)` | Lowest rate covering that much spectrum |
| `diagnose(url)` | `(ok, message, fix)` for each setup check; what `aaronia-doctor` prints |
