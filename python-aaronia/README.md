# python-aaronia

Python bindings for [`sdr-aaronia-rs`](https://github.com/isaacbentley/sdr-aaronia-rs) —
stream IQ samples from Aaronia SPECTRAN V6 devices (via an RTSA-Suite PRO
HTTP server block or the native SDK) or play back recorded `.rtsa` files,
straight into NumPy or Apache Arrow.

- **PyPI package:** `python-aaronia` · **importable module:** `aaronia`
- **Wheels:** abi3, CPython ≥ 3.9, one wheel per OS/arch (plus an sdist
  for everything else — building from source needs a Rust toolchain)
- **License:** GPL-3.0-or-later

## Install

```bash
pip install python-aaronia
```

Or from a checkout (requires Rust + [maturin](https://maturin.rs)):

```bash
cd python-aaronia
maturin develop --release
```

## Quickstart

```python
import aaronia

cfg = aaronia.AaroniaConfig()
cfg.http_base_url = "http://localhost:54664"  # RTSA-Suite HTTP server block
cfg.center_freq = 2.44e9                      # Hz
cfg.sample_rate = 15.36e6                     # Hz
cfg.format = "F32"                            # wire format: F32, F16, or I16

src = aaronia.AaroniaSource()
src.start_streaming(cfg)

samples = src.read_samples_numpy(65536)       # numpy complex64 array
batch = src.read_samples_arrow(65536)         # pyarrow FixedSizeListArray of [re, im]

src.set_center_frequency(2.41e9)              # live retune, no teardown

print(src.cumulative_drops(), src.take_overrun(), src.last_timestamp_ns())
src.stop_streaming()
```

File playback: set `cfg.file_path = "capture.rtsa"` instead of
`http_base_url`.

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
| `format` | HTTP wire format: `"F32"`, `"F16"`, or `"I16"` (true low-bandwidth mode) |
| `receiver_channel` | `"Rx1"` (default), `"Rx2"`, or `"Rx1And2"` (native SDK, full V6) |

Unknown `format`/`receiver_channel` strings raise `ValueError` instead of
silently defaulting.

## Semantics worth knowing

- **One copy per read.** Samples are copied once out of the Rust receive
  buffer into a NumPy/Arrow-owned buffer — safe to hold indefinitely.
  (Not "zero-copy"; one copy is the honest count.)
- **Blocking calls release the GIL.** Other Python threads keep running;
  `KeyboardInterrupt` is delivered between calls. Reads block until
  `count` samples arrive or an internal 30 s timeout raises
  `AaroniaTimeoutError`.
- **Typed exceptions.** `AaroniaConnectionError` (unreachable endpoint),
  `AaroniaTimeoutError`, `AaroniaHardwareError` (device/SDK errors),
  `ValueError` (invalid configuration) — mapped from the Rust error
  enum, with the full cause chain in the message.
- **Dual-channel** (`receiver_channel = "Rx1And2"`,
  `read_samples_dual_numpy(count)` → two time-aligned arrays) requires
  the native-SDK backend: Windows/Linux with the Aaronia SDK installed,
  and a full (two-input) V6. Hardware-unverified — the development
  device is a single-channel V6 ECO.

## Source methods

| Method | Purpose |
| --- | --- |
| `start_streaming(cfg)` / `stop_streaming()` | Session lifecycle |
| `read_samples_numpy(count)` | NumPy `complex64` array |
| `read_samples_arrow(count)` | PyArrow `FixedSizeListArray` of `[re, im]` float32 pairs |
| `read_samples_dual_numpy(count)` | `(rx1, rx2)` NumPy arrays (dual-channel captures) |
| `set_center_frequency(hz)` / `set_sample_rate(hz)` / `set_reference_level(dbm)` | Live retuning |
| `cumulative_drops()` | Total server-reported dropped samples |
| `take_overrun()` | True once per detected receive-side overrun |
| `last_timestamp_ns()` | Epoch-ns timestamp of the last received block (HTTP backend; 0 otherwise) |
