# Hardware verification status

The development device is a SPECTRAN V6 ECO — single RX channel, no TX
licence, no GPS antenna — driven over HTTP through RTSA-Suite PRO from
macOS and, since 0.8.2, through the native SDK on the Windows machine it
is attached to.

**Live-verified** means an `#[ignore]`d test asserts it against hardware,
so it cannot regress unnoticed. **Manual** means it was seen working but
nothing asserts it.

| Capability | HTTP | Native SDK |
| --- | --- | --- |
| IQ streaming (F32 / F16 / I16 / JSON) | **Live** | **Live** |
| Spectra streaming | **Live** | **Live** — the ECO's `rtsa` pipeline, 512 frames x 88 bins per packet |
| Retuning, rate and reference level mid-stream | **Live** | **Live** |
| Long-run stability, drop and overrun counters | **Live** | **Live** |
| Sample-rate ladder and usable bandwidth | **Live** | **Live** |
| Device capability and health reporting | **Live** | Partial — the raw SDK's health tree reads zeros, so sensors are HTTP-only by design |
| Clock-source and GPS-mode writes | **Live** | Unverified |
| Auto-reconnect and connect retry | Mock-tested | n/a |
| End-to-end IQ correctness (`validate-iq-live.py`) | **Live** | — |
| seify backend | **Live** | **Live** |
| Python bindings | Manual | Manual |
| SoapySDR plugin | Manual | Manual |
| `.rtsa` file playback | **Verified against real captures** | — |
| TX (`UnifiedSink`, `spectran_sink_*`) | Endpoint only, no RF measured | Unverified |
| Dual-channel RX (Rust, C, Python, SoapySDR) | — | Unverified — needs a full V6 |
| GPS time (`gps_time_ns`) | — | Unverified — needs a GPS antenna |
| Master stream clock (`master_stream_time_ns`) | — | Unverified |

## What is not verified, and why

**TX, dual-channel RX, GPS time and multi-device sync** need hardware
this project does not have: a TX licence, a full V6, a GPS antenna, or a
second device. If you
have any of them, exercising one of these paths is the most valuable
contribution you can make here — the first native-SDK run against a real
device found five defects that compile-checking never would.

**Native-SDK clock and GPS-mode writes** share their shape with the
HTTP path, which is live-verified, but are not themselves exercised.

**Drops.** The live drop test does not assert that drops *occur* — that
is a property of the link, not the crate, and a fast enough link never
drops anything. It checks the stream stays coherent and the counters
agree; the gap logic itself is unit-tested.

## Running them

```bash
cargo test --all-features --test live_smoke -- --ignored --test-threads=1
```

The native-SDK set needs the machine the device is attached to, with
RTSA-Suite PRO closed — it holds the device:

```bash
cargo test --release --features native-sdk --test native_sdk_live -- --ignored --test-threads=1
```

`--test-threads=1` matters: one live test writes device state, and a
concurrent streaming test would see the reference change underneath it.

Both suites were re-run in full for 0.10.0 — 21 and 14 tests, all
passing — along with a SoapySDR probe through the renamed C ABI.

## Related

- [QUICKSTART.md](QUICKSTART.md) — setting up an RTSA-Suite mission.
- [SYNC.md](SYNC.md) — what multi-device timing the hardware supports.
- [USAGE.md](USAGE.md) — worked examples for each part of the API.
- [../CHANGELOG.md](../CHANGELOG.md) — what changed in each release.
