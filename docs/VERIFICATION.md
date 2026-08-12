# Hardware verification status

Not every code path has been exercised against hardware. The
development device is a SPECTRAN V6 ECO with a single RX channel and no
TX licence, driven through RTSA-Suite PRO over HTTP from macOS. Paths
requiring a second RX input, a transmitter, or the Windows/Linux native
SDK are marked unverified.

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
| SoapySDR plugin RX | HTTP | Verified manually (~9.7 Msps via `SoapySDRUtil`); no automated live test, as a `soapysdr` dev-dependency would make `cargo test` unbuildable without system SoapySDR |
| Python bindings RX | HTTP | Verified manually (NumPy and Arrow); no automated live test |
| TX (`UnifiedSink`, `aaronia_sink_*`, SoapySDR TX) | Native SDK | **Hardware-unverified**. No TX-licensed device available |
| Dual-channel RX (`Rx1And2`, `read_samples_dual`) | Native SDK | **Hardware-unverified**. Requires a full V6. Selects `Rx12`, the interleaved single-stream mode, matching how this crate reads |
| Spectra reads (`read_spectra`) | Native SDK | **Hardware-unverified**. Packet layout and stream index follow Aaronia's `RawSpectrum` sample |
| Device-family detection (`open_detected_device`) | Native SDK | **Hardware-unverified**. Enumerates each known family in turn |
| Sample-rate ladder and usable bandwidth | Both | **Live-verified at the default clock** on a V6 ECO, rung by rung. Faster receiver clocks are inferred from the documented constraint, not measured |
| GPS hardware time | Native SDK | **Hardware-unverified** |
| Native SDK capture generally | Native SDK | **Hardware-unverified**; compiled and unit-tested in a Linux VM each release |
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

Contributions that convert an unverified row, particularly from users
with a full V6 or a TX licence, are welcome.

Native-SDK entries carry a second qualifier worth stating plainly: they
are not merely untested, they run on a platform this project cannot
build on locally. They are compile-checked for Linux and Windows and
their logic is drawn from Aaronia's published samples, which caught
three real defects, but nothing has executed them against a device.

If you have a full V6 or a TX licence, closing one of these rows is the
single most valuable contribution you can make to this crate.

## Related

- [QUICKSTART.md](QUICKSTART.md) — setting up an RTSA-Suite mission.
- [USAGE.md](USAGE.md) — worked examples for each part of the API.
- [../CHANGELOG.md](../CHANGELOG.md) — what changed in each release.
