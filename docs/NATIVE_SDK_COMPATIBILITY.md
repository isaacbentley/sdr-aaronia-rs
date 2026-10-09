# Native SDK configuration and qualification (0.13)

The driver queries the opened device and mode rather than imposing one ECO's
rate ladder on every SPECTRAN. `DeviceCapabilities` reports SDK configuration
metadata; it does not certify every model, hardware option or licensed mode.
Aaronia's [current compact USB range](https://aaronia.com/en/shop/spectrum-analyzer/real-time/compact-usb)
has different bandwidths and receiver/transmitter options. Consult the specific
model's rating as well as the SDK's enabled options.

## Supported receive paths

| Family / mode | Configuration path | Validation here |
| --- | --- | --- |
| `spectranv6eco/iqreceiver` (automatic ECO default) | RF span via `main/spanfreq`; actual rate/span from IQ packets | Windows ECO mono live trial |
| `spectranv6/iqreceiver` | RF span via `main/spanfreq`; actual rate/span from IQ packets | Mode/geometry tests; no hardware trial |
| `spectranv6/raw`, `spectranrsa/raw` | Raw clock, channel and decimation options from ConfigInfo | Configuration/parser tests; no hardware trial |
| `spectranv6eco/raw` (explicit only) | Enabled raw clock/channel/decimation options when exposed | Mode tests; no dual-ECO hardware trial |
| ECO `rtsa`, V6/ECO `sweepsa` | Low-level spectra API only; refused by unified IQ configuration | Payload guards |
| Other family/mode strings | Enumeration may accept an explicit family; unknown IQ payload modes are refused | No hardware claims |

Automatic discovery tries the known family names. `DeviceNotFound` and
`DeviceNotReady` are typed errors; SDK ownership errors retain `SdkApi` codes.
The application chooses its retry deadline. No retry silently changes device,
starts RTSA, or takes over a device owned by another process.

## Three different bandwidth values

1. `NativeReceiverOptions.rf_span_hz` / `set_rf_span_hz`: requested RF span in an
   IQ receiver mode. The setter returns SDK read-back while open, or the queued
   request while stopped; restart revalidates deferred settings.
2. `DeviceCapabilities.rf_span_hz`: SDK ConfigInfo setting bounds. The tested ECO
   SDK returned 1 Hz–200 MHz, exceeding that model's 44 MHz rated width. These
   bounds alone must not be presented as a calibrated hardware bandwidth.
3. `capture_bandwidth_hz()` / `SourceInfo.bandwidth_hz`: valid RF span declared by
   the IQ packets actually returned. `capture_sample_rate_hz()` is the separate
   IQ sample clock. Unavailable capture bandwidth is zero (unknown).

RF span does not establish dBm amplitude calibration. The legacy HTTP 0.8×Fs
helper and the IQ receiver's legacy Fs/1.5 request translation are compatibility
helpers, not universal hardware laws. Use explicit RF span and observed packet
geometry when the recording or display depends on exact coverage.

## Explicit raw configuration

Select `mode=raw`, then use an enabled `receiver_clock` label and/or a power-of-two
`decimation_factor`. The option's original index is retained despite disabled or
empty entries. An unavailable/disabled request fails before configuration writes.
Explicit raw clock/decimation selects the pipeline instead of the legacy
`sample_rate_hz` request. The delivered packet rate remains authoritative.
Receiver channels are validated against the opened raw mode. A mono ECO trial
cannot establish dual-RX support, interleaving, or the licensed options of a
200-series ECO, PLUS or XPR device.

## Opening, stopping and reopening

Partial open/connect/start failures attempt all applicable cleanup. Stop attempts
stop, disconnect and close, returning the first error after attempting the rest.
A failed close remains owned for a Drop retry. Repeated start/stop calls are
idempotent; successful stop clears stale carry/geometry. Unified restart reopens
the same serial and mode, reapplying settings. Settings queued while stopped are
checked against fresh metadata before writes on reopen. SDK calls themselves
remain vendor operations without a driver-imposed cancellation deadline.

## Binding parity

Rust exposes typed capabilities/options. C adds capability JSON (schema version
1), RF span range/setter and builder options without changing existing C struct
layouts; free JSON with `spectran_string_free`. Python exposes config fields,
`capabilities()`, `source_info()` and `set_rf_span_hz()`. Seify and Soapy consume
reported bounds; an unknown native discrete rate ladder is empty rather than a
fabricated ECO ladder. Soapy's native bandwidth is observed RF span, not Fs×0.8.
Soapy TX setup now requires explicit `tx=true` and compatible licensed hardware.

## Windows live result, 2026-10-08

An explicitly selected SPECTRAN V6 ECO on Windows, SDK API 1.0.4, was tested with
RTSA closed, reference level −50 dBm, 65,536-sample reads and five eight-second
cycles. Each cycle called start twice, read IQ, then called stop twice. Frequency
and reference settings were queued while closed; restart reopened the same serial.

| Cycle | Requested / observed RF span | Observed IQ rate | Centre | Returned samples |
| --- | --- | --- | --- | --- |
| 1 | 44 MHz | 59.2137035 MS/s | 280 MHz | 268,500,992 |
| 2 | 20 MHz | 30 MS/s | 281 MHz | 196,476,928 |
| 3 | 10 MHz | 15 MS/s | 282 MHz | 98,435,072 |
| 4 | 5 MHz | 7.5 MS/s | 283 MHz | 49,119,232 |
| 5 | 44 MHz | 59.2137035 MS/s | 284 MHz | 388,038,656 |

The program exited successfully and printed its completion marker. The cumulative
discontinuity count was one from the first cycle and did not increase. Signals
was restored afterward with fresh samples and its previous tune/reference level.
This short mono trial does not establish endurance, hot-unplug recovery, Linux
hardware operation, wider raw clocks, dual-RX, TX or all-model compatibility.

Reproduce only on a device available for exclusive receive use:

```sh
cargo run --example native_sdk_qualification --features native-sdk -- YOUR_DEVICE_SERIAL 8
```
