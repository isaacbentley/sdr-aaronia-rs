# Hardware verification status

Not every code path has been exercised against hardware. The
development device is a SPECTRAN V6 ECO with a single RX channel and no
TX licence, driven through RTSA-Suite PRO over HTTP from macOS and,
since 0.8.2, through the native SDK on a Windows 11 machine it is
attached to. Paths requiring a second RX input, a transmitter, or a
full V6 are marked unverified.

| Capability | Backend | Status |
| --- | --- | --- |
| IQ streaming, all four wire formats (F32 / F16 / I16 / JSON) | HTTP | **Live-verified** |
| Spectra streaming | HTTP | **Live-verified** |
| Mid-stream retuning (centre, span) | HTTP | **Live-verified** |
| Mid-stream reference-level change | HTTP | Confirmed manually against the device; no automated live assertion |
| Auto-reconnect after a dropped stream | HTTP | Streaming live-verified; the drop-and-recover path is mock-tested |
| Drop/overrun detection, rate reduction, `scale=N` | HTTP | **Live-verified** |
| Long-run stability (>120 s continuous) | HTTP | **Live-verified** |
| Connect retry | HTTP | Mock-tested; the mDNS race it addresses did not reproduce on demand |
| `.rtsa` playback and metadata | File | **Verified against real captures**, byte-compared with the official format specification |
| seify backend | HTTP | **Live-verified** |
| SoapySDR plugin RX | HTTP | Verified manually: CF32 and CS16 both sustained 15.36 MS/s, the full requested rate, over an 8 s window after the connect backlog. No automated live test, as a `soapysdr` dev-dependency would make `cargo test` unbuildable without system SoapySDR |
| SoapySDR device reporting (model, serial, ranges, rate ladder, clock source, antenna, timestamps, channel counts) | HTTP | Verified manually against a V6 ECO: every field the probe prints comes from the device, `getAntenna` is a member of `listAntennas`, and `getClockSource` a member of `listClockSources` |
| Python bindings RX | HTTP | Verified manually: 15.58 MS/s of live IQ into NumPy, mean power matching the SoapySDR path on the same signal to within 3%. Also Arrow. No automated live test |
| TX (`UnifiedSink`, `aaronia_sink_*`, SoapySDR TX) | Native SDK | **Hardware-unverified**. No TX-licensed device available |
| Dual-channel RX (`Rx1And2`, `read_samples_dual`) | Native SDK | **Hardware-unverified**. Requires a full V6. Selects `Rx12`, the interleaved single-stream mode, matching how this crate reads |
| Spectra reads (`read_spectra`) | Native SDK | **Hardware-unverified**. Packet layout and stream index follow Aaronia's `RawSpectrum` sample |
| Device-family detection | Native SDK | **Live-verified** on a V6 ECO: `spectranv6` enumerates nothing, `spectranv6eco` finds it, and the source opens `spectranv6eco/iqreceiver` |
| Sample-rate ladder and usable bandwidth | Both | **Live-verified at the default clock** on a V6 ECO, rung by rung. Faster receiver clocks are inferred from the documented constraint, not measured |
| End-to-end IQ correctness | HTTP | **Live-verified** by `scripts/validate-iq-live.py`: every wire format decodes to the same spectrum, the Python and SoapySDR paths agree, and a NOAA weather-radio carrier lands within 312 Hz of its known frequency on the correct side of zero |
| Device capability reporting (`get_device_capabilities`) | HTTP | **Live-verified** — the declared centre-frequency and reference-level bounds, the decimation ladder, the clock-source list and the RX input |
| Stream-gap device-health cross-check (`get_device_health`) | HTTP | **Live-verified** — the device's own loss counters parse and yield a verdict |
| Link budget and throughput probe (`link_budget`) | HTTP | Measured manually over gigabit and 2.5GbE, rung by rung; no automated live assertion |
| Several clients on one server block | HTTP | Measured manually at one, two and five concurrent clients; no automated assertion |
| GPS hardware time | Native SDK | **Hardware-unverified** |
| Native SDK library load and symbol resolution | Native SDK | **Verified against the real library** on both platforms — 3.0.3.16655's `libAaroniaRTSAAPI.so` in an x86-64 Linux container (`scripts/sdk-container-test.sh`) and `AaroniaRTSAAPI.dll` on Windows 11 from a stock install, which needed the install root on the DLL search path. All 34 symbols resolve; `AARTSAAPI_Version()` answers 1.4 |
| Native SDK IQ capture (single channel) | Native SDK | **Live-verified** on a V6 ECO over USB on Windows 11 by `tests/native_sdk_live.rs`: opens, ten open/close cycles, six centre frequencies, mid-stream retune, every ladder rung 3.84–49.152 MHz delivering exactly the requested rate (61.44 caps at the 59.2 MS/s USB ceiling), 100.0% steady-state delivery at 15.36 MS/s, and a 30 s soak with the device's own drop flags counted |
| Native SDK from Python (`aaronia.open(sdk=True)`) | Native SDK | **Verified manually** on the same machine against radioconda's Python 3.12: 15.363 MS/s of a 15.36 MS/s request into NumPy, retune, timestamps and drop counters populated |
| Native SDK from the C ABI | Native SDK | **Live-verified**: a builder with neither URL nor file auto-detects the SDK; there is still no explicit selector over C |
| Native SDK from Seify (`sdk=true`) | Native SDK | Compiled with `--features seify,native-sdk`; the live test exists but was not run on this pass |
| SoapySDR plugin over the native SDK | Native SDK | **Verified manually** on Windows 11: built with MSVC (VS 2022 Build Tools) against radioconda's SoapySDR 0.8.1, loads into it, and streams 15.357 MS/s of a 15.36 MS/s request into Python via `driver=aaronia,serial=…` (`SoapySDR.Device("driver=aaronia,serial=…")` — that build's binding rejects the `dict` form for every driver). The plugin reaches the SDK only through `serial=`; a bare `driver=aaronia` still pins to localhost HTTP. In the C++ host `SoapySDRUtil`, which sits beside radioconda's own Qt6 copies, the SDK library failed to load: the loader searched the host's directory ahead of the SDK's, fixed after 0.8.2 |
| HTTP TX push (`/sample`) | HTTP | Endpoint exercised live; RF output not measured |

"Live-verified" means an `#[ignore]`d test in
[`tests/live_smoke.rs`](../tests/live_smoke.rs) asserts the behaviour
against hardware, and is reproducible by anyone with a device. Entries
marked "verified manually" were observed working but have no automated
assertion and can regress without detection. Run the automated set
with:

```bash
cargo test --all-features --test live_smoke -- --ignored --nocapture
```

and the native-SDK set, on a Windows or Linux machine the device is
attached to (RTSA-Suite PRO closed — it holds the device):

```bash
cargo test --release --features native-sdk --test native_sdk_live -- --ignored --nocapture --test-threads=1
```

Contributions that convert an unverified row, particularly from users
with a full V6 or a TX licence, are welcome.

The native-SDK rows still marked unverified — TX, dual-channel RX,
spectra reads, GPS time — need a full V6 or a TX licence. Single-channel
IQ through the SDK is no longer among them: the first run against a
device found five defects that no amount of compile-checking had, which
is the argument for closing the remaining rows the same way. If you have
a full V6 or a TX licence, that is the single most valuable contribution
you can make to this crate.

## Related

- [QUICKSTART.md](QUICKSTART.md) — setting up an RTSA-Suite mission.
- [USAGE.md](USAGE.md) — worked examples for each part of the API.
- [../CHANGELOG.md](../CHANGELOG.md) — what changed in each release.
