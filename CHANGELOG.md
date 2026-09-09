# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- **The GPS mode can be set, not just the clock source.** `device/gpsmode`
  decides whether GPS supplies location, time, both, or nothing, and the device
  ships on `Disabled` — in which state no fix is ever reported and
  `gps_time_ns` returns `None` forever, reading as broken rather than
  unconfigured. `SpectranSource::set_gps_mode` writes it on both backends,
  with the same confirm-by-read-back rule as the clock source.
- **`clock_source()`, `clock_sources()`, `gps_mode()`, `gps_modes()`** on
  `SpectranSource`, so reading the current reference no longer means fetching
  the whole capability tree. These read over HTTP; the writes work on both
  backends.

### Fixed
- **Reading the wrong payload for the open mode is refused instead of answered.**
  Neither the SDK nor the packet says what a stream carries, so asking a
  spectrum pipeline for IQ returned dBm bins reinterpreted as voltages, and
  asking an IQ pipeline for spectra returned complex pairs reinterpreted as
  two-bin frames whose bin spacing was the whole span. Both look like data.
  Measured on a V6 ECO's `rtsa`: 200k "samples" spanning -130 to -59 with a
  mean of -77 and not one value near zero — a dBm distribution, not a voltage
  one. `read_samples`, `read_samples_dual` and `read_spectra` now check the
  open mode and say which payload it carries.

  This also corrects a doc claim: `spectranv6eco/rtsa` was documented as
  yielding IQ at "~0.4 MS/s regardless of the requested rate". It yields no IQ
  at all; that figure was the spectra misread. `spectranv6/raw` genuinely does
  both, IQ on stream 0 and spectra on stream 2, and is unaffected.
- **Capability lists no longer advertise options the device will refuse.** The
  config tree carries a bitmask of enum entries the device is currently
  rejecting, and it was ignored: a V6 ECO with no GPS antenna offered `GPS` and
  `GPS Provider` as clock sources, and `set_clock_source("GPS")` then failed
  against a list that said it would not. Those entries are filtered out, so what
  `clock_sources()` and `gps_modes()` return is what the device accepts.
  Verified live: the six sources advertised are exactly the six it takes.

## [v0.10.0] - 2026-09-08

### Breaking changes

**One name per concept, with its unit.** A field, parameter or getter holding a
bare number now carries its unit (`_hz`, `_dbm`, `_db`, `_s`); a type that
already carries one, such as `Duration`, does not. `span_frequency` is gone: it
meant the IQ *sample rate*, while the same word meant alias-free *bandwidth* in
the link budget. Old names were removed rather than deprecated, since an alias
that kept working would preserve the ambiguity this release exists to remove.
The `spanfreq` device key and the `frequencySpan` JSON field are the vendor's
vocabulary and are unchanged.

| Old | New |
| --- | --- |
| `AaroniaConfig::center_frequency` (field + builder) | `center_frequency_hz` |
| `AaroniaConfig::span_frequency` (field + builder) | `sample_rate_hz` |
| `AaroniaConfig::reference_level` (field + builder) | `reference_level_dbm` |
| `AaroniaSourceBuilder::center_frequency` / `span_frequency` / `reference_level` | `center_frequency_hz` / `sample_rate_hz` / `reference_level_dbm` |
| `AaroniaSource::set_center_frequency` / `set_span_frequency` / `set_reference_level` | `set_center_frequency_hz` / `set_sample_rate_hz` / `set_reference_level_dbm` |
| `SourceInfo::center_frequency` / `span_frequency` / `reference_level` | `center_frequency_hz` / `sample_rate_hz` / `reference_level_dbm` |
| `SourceInfo::sample_rate_hz()` (method) | removed — read the field of the same name |
| `UnifiedSinkConfig::span_frequency`, `trans_gain` | `sample_rate_hz`, `trans_gain_db` |
| `ThroughputMeasurement::max_sustainable_span_hz` | `max_sustainable_bandwidth` |
| `ThroughputMeasurement::stream_sample_rate` | `stream_sample_rate_hz` |
| `LinkBudgetVerdict::sample_rate`, `fit_span_hz` | `sample_rate_hz`, `fit_bandwidth_hz` |
| `RtsaMetadata` / `StreamingSdrConfig` bare quantities | unit-suffixed (`_hz`, `_s`) |
| `HttpSourceBuilder::frequency` / `frequency_str` / `sample_rate` / `reference_level` | `center_frequency_hz` / `center_frequency_str` / `sample_rate_hz` / `reference_level_dbm` |
| `HttpSinkBuilder::frequency`, `sample_rate` | `center_frequency_hz`, `sample_rate_hz` |
| `HttpSource::new` / `with_advanced_options`, `HttpSink::new` parameters | `center_frequency_hz` / `sample_rate_hz` / `reference_level_dbm` |
| `DeviceCapabilities::center_frequency`, `reference_level` | `center_frequency_hz`, `reference_level_dbm` |
| `PacketMetadata::sample_rate()` | `sample_rate_hz()` |

`frequency` became `center_frequency_hz`, not `frequency_hz`: it was always the
centre frequency. The `_str` variants take a string carrying its own units
(`"146.52M"`), so they keep no suffix. `DeviceCapabilities` is
`#[non_exhaustive]`, so callers reach those fields through the API rather than
by literal.

**Rust types name the device: `Aaronia*` becomes `Spectran*`** —
`SpectranConfig`, `SpectranSource`, `SpectranSourceBuilder`,
`SpectranSinkBuilder`, `SpectranSeifyDevice`, `SpectranSeifyRxStreamer`,
`SpectranBackend`, `SpectranSdrSource`.

The vendor name stays wherever it is a frozen contract rather than a
description: the `aaronia_*` C symbols and the `AaroniaSource` /
`AaroniaFfiError` typedefs, the crate, the PyPI package, the Python module,
`AaroniaSoapyDevice`, and `driver=aaronia`. That last is the load-bearing one —
it is typed into GQRX and SDR++ configs already in the world, where breaking it
reads as "device not found" with nothing pointing at the cause.

**Python takes the same vocabulary**, its four exceptions included.

| Old Python | New |
| --- | --- |
| `aaronia.AaroniaConfig` | `aaronia.SpectranConfig` |
| `aaronia.AaroniaSource` | `aaronia.SpectranSource` |
| `aaronia.Aaronia{Connection,Hardware,Timeout}Error`, `AaroniaStreamClosed` | `Spectran…` |
| `cfg.center_freq` | `cfg.center_frequency_hz` |
| `cfg.sample_rate` | `cfg.sample_rate_hz` |
| `cfg.reference_level` | `cfg.reference_level_dbm` |
| `cfg.read_timeout` | `cfg.read_timeout_s` |
| `cfg.native_sdk` | `cfg.force_native_sdk` |
| `src.set_center_frequency()` / `set_sample_rate()` / `set_reference_level()` | `set_center_frequency_hz()` / `set_sample_rate_hz()` / `set_reference_level_dbm()` |
| `open(freq=, rate=, bandwidth=, ref_level=, read_timeout=)` | `open(center_frequency_hz=, sample_rate_hz=, bandwidth_hz=, reference_level_dbm=, read_timeout_s=)` |

`open()`'s keywords changed with them. It is the one-line front door and the
longer names cost the headline example a wrap, but `open(url, bandwidth=10e6)`
gives no way to tell Hz from MHz. `sdk=`, `url=`, `file=`, `serial=`, `format=`
and `scale=` carry no unit and are unchanged.

**The whole C ABI moves to `spectran_*`.** Every exported symbol, the header
(`include/aaronia.h` is now `include/spectran.h`), the opaque typedefs
(`AaroniaSource` / `AaroniaSourceBuilder` / `AaroniaSink` / `AaroniaSinkBuilder`
/ `AaroniaFfiError` / `CAaroniaSourceType` become `Spectran*`), and the plugin
class `AaroniaSoapyDevice` become `SpectranSoapyDevice`. The table below lists
the symbols that also changed shape; the rest changed prefix only, so
`aaronia_x` is `spectran_x`.

What keeps the vendor name: `driver=aaronia`, because it is typed into GQRX and
SDR++ configuration files already in the world and breaking it reads as "device
not found"; the crate, the PyPI package and the Python module, which are
published names; and `AARTSAAPI_*` / `AaroniaRTSAAPI.dll`, which are the
vendor's own.

**The C ABI is renamed with no forwarders**, so a consumer gets an undefined
symbol at link time and this table is the migration guide. `FfiSourceInfo`'s
fields moved with the header in the same commit: C reads them positionally, so
a stale header would go on reading the right bytes under the wrong name.

| Old C symbol | New |
| --- | --- |
| `aaronia_source_builder_center_frequency` | `spectran_source_builder_center_frequency_hz` |
| `aaronia_source_builder_span_frequency` | `spectran_source_builder_sample_rate_hz` |
| `aaronia_source_builder_reference_level` | `spectran_source_builder_reference_level_dbm` |
| `aaronia_source_set_center_frequency` | `spectran_source_set_center_frequency_hz` |
| `aaronia_source_set_span_frequency` | `spectran_source_set_sample_rate_hz` |
| `aaronia_source_set_reference_level` | `spectran_source_set_reference_level_dbm` |
| `aaronia_sink_builder_center_frequency` | `spectran_sink_builder_center_frequency_hz` |
| `aaronia_sink_builder_sample_rate` | `spectran_sink_builder_sample_rate_hz` |
| `aaronia_sink_builder_trans_gain` | `spectran_sink_builder_trans_gain_db` |
| `aaronia_source_read_sensors` | `spectran_source_get_sensors` |
| `aaronia_endpoints_client_read_sensors` | `spectran_endpoints_client_get_sensors` |
| `FfiSourceInfo::center_frequency` / `span_frequency` / `reference_level` | `center_frequency_hz` / `sample_rate_hz` / `reference_level_dbm` |
| `aaronia_source_get_gps_time(.., double*)` | `spectran_source_get_gps_time_ns(.., int64_t*)` |

The two `read_sensors` are a verb change, not a unit one. `read_` advances a
stream here (`spectran_source_read_samples`); sensors are a snapshot, so they
join `spectran_source_get_capabilities`.

**GPS time changes type as well as name.** `gps_time_ns() -> Option<i64>`, and
`int64_t*` in C, matching `last_timestamp_ns`. The conversion moved into the
library because epoch nanoseconds land near `1.7e18`, where an `f64`'s step is
256 ns — a single `seconds * 1e9` discards tens of nanoseconds the reading
still had. The SoapySDR plugin's own whole/fractional split is deleted.
`GpsState::time` is now `time_s`, and the C function accepts a `NULL`
out-parameter meaning "is there a fix?".

It does **not** improve precision. The device reports `gpstime` as an `f64`,
whose step at present-day epoch values is about 240 ns, and nothing downstream
recovers what was never there. `utils::gps_seconds_to_nanos` documents that
bound and a test pins it against the naive multiply. Across receivers ~240 ns
is ~70 m of ranging uncertainty, so correlate on the per-packet stream
timestamps and keep GPS for disciplining and wall-clock labelling.

`CaptureControl`'s fields gained units without moving its JSON keys: each is
now pinned by an explicit `#[serde(rename)]` rather than derived from the Rust
field name, so a later rename cannot change the wire format by accident.
`tests/wire_contract.rs` asserts those keys, and asserts the vendor
`AARTSAAPI_Packet` mirror still matches its C header field for field.

### Added
- **The stream clock source can be set, not just read.** `device/sclksource`
  selects what disciplines the receiver's clock; a V6 ECO offers `Consumer`,
  `Oscillator`, `GPS`, `PPS`, `10MHz` and three `... Provider` variants. It was
  readable but read-only, and SoapySDR's `setClockSource` merely logged a
  warning telling the operator to change it in RTSA-Suite.
  `SpectranSource::set_clock_source`, `spectran_source_set_clock_source` and the
  plugin's `setClockSource` now write it on both backends — native SDK via
  `ConfigSetString`, HTTP via a `simpleconfig` PUT that is read back to confirm,
  because a `/remoteconfig` PUT naming a block outside the running mission
  answers 200 and changes nothing. A confirmed mismatch is an **error** naming
  the sources the device does offer: being told you are on a reference you are
  not is worse than a failed call. A read-back yielding nothing is reported as
  unconfirmed rather than failed.

  This is what makes cross-receiver correlation possible: lock a fleet to one
  10 MHz / PPS / GPS reference and their per-packet timestamps share a timebase.
  What is *not* possible is a commanded synchronous start — the SDK's C API has
  no set-time and no arm-at-time entry point, so multi-device work is a shared
  reference plus post-alignment on timestamps.

### Fixed
- **A trailing slash in `device_type` no longer produces a malformed open
  string.** The family/mode split asked whether the string contained a slash, so
  `"spectranv6/"` counted as already mode-qualified and went to
  `AARTSAAPI_OpenDevice` verbatim. The mode is now the text after the *first*
  slash, and an empty one means none was given. The ECO was the sharp edge:
  `"spectranv6eco/"` also slipped past the `spectranv6eco/raw` → `iqreceiver`
  remap, so the device was asked for a pipeline it does not have and answered
  with a result code that said nothing about the typo.

  A `device_type` naming no family (`""`, `"/"`, `"/raw"`) is no longer
  completed into a family-less `"/raw"`; it comes back untouched and is refused
  at both points where a device type reaches the SDK. Both guards are needed
  because enumeration runs first, and an empty family simply enumerates to
  nothing — the old path reported "no devices found" and never mentioned the
  typo. `device_family` and `device_open_mode` stay infallible: a typo in one
  field is not a reason to make four accessors return `Result`.
- **An over-wide sample rate no longer reaches the hardware before it is
  refused.** `configure_iq_receiver` checked the IQ-mode constraint as its last
  act, so a rate the receiver clock cannot carry was written to
  `main/centerfreq` and `main/spanfreq` first and rejected afterwards — leaving
  the device holding the misconfiguration the check exists to prevent, and per
  the SDK quietly emitting corrupted samples if anything started the stream.
  The check now runs before the first write.

  It checks against the clock the call *leaves in place*, which is not always
  the one the device holds on entry. Raw mode writes the clock itself, so that
  write is what counts: a V6 left on its 245.76 MHz clock would otherwise pass a
  150 MS/s request that stops being valid the moment this same call drops the
  clock to 92.16 MHz. Every other mode skips that write, so there the live
  setting is the honest number. The read-back after the writes is kept, and is
  what `receiver_clock_hz()` reports. Native SDK only; the HTTP backend already
  validated at the API boundary.

## [v0.9.0] - 2026-09-08

### Added
- **SoapySDR: device sensors.** The plugin exposed one sensor
  (`cumulative_drops`); it now surfaces the device's live telemetry from
  `/healthstatus` — FPGA and frontend temperature, ADC headroom (dB below
  full scale, so a client can raise the reference level before it clips),
  USB and DSP buffer fill, error and overflow rates, GPS satellites and
  position. `listSensors` / `readSensor` / `getSensorInfo`, read through a
  dedicated connection and briefly cached, so polling them during a
  capture neither stalls the sample stream nor costs a fetch per key.
  HTTP backend only: the native SDK's own `AARTSAAPI_ConfigHealth` tree
  reads all zeros in raw-SDK mode — the live telemetry is computed and
  populated by RTSA-Suite, the managing application, not by the raw SDK,
  confirmed by dumping the tree live from a V6 ECO — so over `sdk=true`
  the plugin reports only `cumulative_drops`, which is real and
  client-side. Verified live over HTTP on a V6 ECO.
- **SoapySDR: bandwidth API.** `setBandwidth` / `getBandwidth` /
  `listBandwidths` / `getBandwidthRange`. The device's alias-free
  bandwidth is 0.8x its sample rate; these expose that as SoapySDR's
  separate knob, mapping a requested bandwidth to a sample rate and
  driving the already-verified `setSampleRate` — no new device-write path.
  Verified on a V6 ECO: 15.36 MS/s reports 12.288 MHz, `setBandwidth`
  snaps to a rung.
- **SoapySDR: the native SDK is discoverable.** `find` advertises a
  second `sdk=true` device beside the HTTP one whenever the SDK is
  installed, and `sdk=true` / `serial=` force the native backend instead
  of silently falling back to localhost HTTP. Verified streaming 15.357
  MS/s over the SDK through the plugin from Python.
- **C ABI.** `aaronia_source_read_sensors` (fills a value struct, `NaN`
  for absent — no ownership, nothing to free);
  `aaronia_source_builder_force_source_type` and `aaronia_sdk_installed`,
  the C equivalent of `force_native_sdk`, which C had no way to request;
  and the stateless bandwidth helpers `aaronia_usable_bandwidth_hz` /
  `aaronia_iq_sample_rate_for_bandwidth`. `DeviceSensors` is the Rust type
  behind the first.

### Fixed
- **File playback reported the decompression time as the capture time.**
  A DSPT_IQ-compressed `.rtsa` is decompressed through RTSAFileTool, which
  writes a fresh header stamped with the moment of conversion, so the
  re-opened file reported *now* — a 2020 capture read as 2026. The
  original header's `creation_time`, parsed before compression was
  detected, is now carried across the decompression, and the derived
  start/end fall-backs follow it. Surfaced the first time the fixture test
  ran on a machine with RTSA-Suite installed.
- **The SoapySDR plugin could not load the SDK inside a host that carries
  its own Qt6.** The loader was given `LOAD_LIBRARY_SEARCH_DEFAULT_DIRS`,
  which searches the host application's directory first; a GNU Radio
  (radioconda) install holds a different Qt6 build there under the same
  names, and the load failed. It now searches only the DLL's own
  directory, the SDK install root, and System32.
- **The Seify native-SDK test panicked on teardown.** `AaroniaSeifyDevice`
  owns a tokio runtime, and dropping one inside an async context is a
  tokio panic; the test is synchronous now.

## [v0.8.2] - 2026-09-08

The native-SDK backend, validated on a Spectran V6 ECO on Windows 11 for
the first time. Every item below was found by running against the device;
the suite that found them ships as `tests/native_sdk_live.rs`.

### Fixed

- **The SDK library could not load on a stock Windows install.** RTSA-Suite
  PRO puts `AaroniaRTSAAPI.dll` in `sdk\` and its ~23 dependencies (Qt6,
  avcodec, libcrypto, …) in the install root; `LoadLibraryExW` searches
  neither, so detection found the file and the load failed with a bare
  `LoadLibraryExW failed`. The install root is added to the DLL search list
  (`AddDllDirectory`) and the library loaded with the
  `LOAD_LIBRARY_SEARCH_*` flags that consult it — not `SetDllDirectory`,
  which would switch safe search mode off for the whole host process.
- **A V6 ECO was never found.** `init_native_sdk` enumerated the family
  `spectranv6` only; the ECO answers to `spectranv6eco`, so a machine
  holding one reported "No Spectran V6 devices found" with the device on
  the bus. Each known family is now tried, as `detect_device_family`
  already documented.
- **ECO IQ came from the spectrum pipeline at 0.4 MS/s.** The ECO's raw-IQ
  mode was mapped to `spectranv6eco/rtsa`, which is its spectrum pipeline
  (`RawSpectrumEco.cpp`); IQ read from it arrived at ~0.4 MS/s whatever
  rate was asked for. `spectranv6eco/iqreceiver` — the mode Aaronia's own
  `IQReceiverEco.cpp` opens — is used instead. `/raw` also opens on an ECO
  but has no `main/spanfreq`, so it cannot honour a requested rate.
- **The ECO delivered 1.5× the requested rate.** In `iqreceiver` mode
  `main/spanfreq` is a bandwidth and the pipeline streams at 1.5× it —
  every rung exactly, 10 MHz → 15.0 MS/s, 15.36 → 23.04 — up to a
  59.2 MS/s USB ceiling. The request is now translated so the caller gets
  the rate it named on every backend: a 15.36 MS/s request measures
  15.360 MS/s over 153 M samples.
- **Native-SDK loss was invisible.** Packet flags `WARN_DROPPED` and
  `TIME_DISCONTINUITY` were logged at debug level and nothing else, so
  `cumulative_drops()` read 0 through any amount of loss and
  `last_timestamp_ns()` read 0 always. Both, and `take_overrun()`, now
  report the native source's packets. `WARN_INACCURATE` is deliberately
  not an overrun: the ECO's fractional resampler sets it routinely.
- **`sample_rate_hz()` echoed the request on native sources.** It now
  reports the rate the device's packets carry once one has been read, as
  the HTTP backend already did.
- **`read_samples` and `read_samples_dual` returned one packet per call**
  however large `max_samples` was. Both now drain already-queued packets
  until the caller is satisfied, waiting only for the first — and not
  even that when carry-over samples were already handed out, where the
  old code could sleep out the whole poll deadline on a backlog.
- **A `device_serial` in the second family was never found.** Family
  enumeration stopped at the first family holding any device, so a
  machine with both a V6 and an ECO could not select the ECO by serial.
  Every family is enumerated and the serial resolved across them.
- **Python could not ask for the SDK.** `python-aaronia` depended on the
  crate's default features, which exclude the backend, and `aaronia.open()`
  with neither `url` nor `file` pinned to localhost HTTP rather than
  auto-detecting. A `native-sdk` feature and `open(sdk=True, serial=…)` /
  `AaroniaConfig.native_sdk` select it explicitly; a missing SDK is then
  an error, never a silent fallback.
- **A unit test assumed no SDK on the machine.**
  `test_detect_best_source_type_localhost_fallback` asserted the
  localhost-HTTP fallback unconditionally, so `cargo test` failed on any
  machine with RTSA-Suite installed — the one place the backend gets
  tested. It now asserts the actual rule: the SDK when installed, HTTP
  otherwise.
- **The Soapy plugin's Windows build assumed MSVC.** The Rust static-lib
  name is keyed on the compiler now, not the OS; the windows-gnu toolchain
  emits `libsdr_aaronia_rs.a`.

### Added

- `tests/native_sdk_live.rs`: thirteen hardware tests — detection,
  identity, ten open/close cycles, a centre-frequency sweep, mid-stream
  retune, a span-ladder characterisation that records the device-reported
  rate per rung, a steady-state rate check, a soak with drop accounting,
  error-message quality, and the Seify and C-ABI paths.
- `scripts/native-sdk-validate.ps1`: runs the whole matrix on a Windows
  machine with a device.
- `NativeSdkSource::{observed_sample_rate_hz, cumulative_drops,
  take_overrun, last_timestamp_ns}`.

### Measured on a V6 ECO (Windows 11, RTSA-Suite PRO 3.0.3)

Every ladder rung from 3.84 to 49.152 MHz delivers exactly the requested
rate, 3/3 trials each; 61.44 MHz caps at 59.214 MS/s. Steady-state
delivery is 100.0% of the reported rate. The `iqreceiver` pipeline
delivers ~40% of rate for the first ~5 s after start, then settles; the
rate and soak tests warm up past it. The SoapySDR plugin, built with
MSVC against radioconda's SoapySDR, streams 15.357 MS/s over the SDK
into Python; see [docs/VERIFICATION.md](docs/VERIFICATION.md) for the
two limits found on the way.


_Also in 0.8.2 (recorded here on the 0.9.0 pass; these landed across the 0.8.x docs/perf commits and were never restamped):_

### Performance
- **The int16 and float16 IQ decoders are ~23% faster end to end.** Both
  built their output with `Vec::with_capacity` then `push` per sample,
  which carries a capacity check the compiler cannot elide and which was
  blocking vectorisation. Collecting from the slice iterator instead —
  `TrustedLen`, so the vector is sized once — measured 495 to 2433 MS/s
  on the decode loop alone, and 123.5 to 152 MS/s over the whole HTTP
  path including framing and transport. float32 was already a single
  `copy_nonoverlapping` and is unchanged.

  Worth stating what this does not buy: at 605 MB/s the crate is about
  2.5x the fastest a V6 can produce and 6x a WiFi 6E link, so it was not
  the bottleneck before and is not now. What it buys is CPU left over
  for whatever consumes the samples.

### Added
- **`scripts/fake-rtsa-server.py`** serves `/stream` in the real wire
  format at loopback speed, so a decode path can be measured without a
  device or a network. It is how the figures above were taken: over any
  real link the transport dominates and a CPU change is invisible. Its
  docstring carries the caveat that goes with it — over loopback hyper's
  adaptive read buffer never leaves its 8 KiB floor, so kernel time there
  is a property of the harness, not of the crate.

### Documentation
- **`rate_reduction` is time compression, not a sample-rate divider.**
  Five places described it as reducing the rate or optimising bandwidth.
  It thins frames over time — the operation the `waterfall` payload is
  described by — so a continuous IQ stream, having no frames, is
  unaffected: measured at factors of 2, 10 and 64, `sampleFrequency`
  holds at 15,359,988 Hz and the byte rate does not move. That is the
  parameter behaving as specified on a payload it was not meant for.
  `live_stream_rate_reduction_and_scale` had asserted only that packets
  arrived, which is true either way, so nothing caught the wrong
  description; it now pins the IQ behaviour.
- **The stream can be compressed, up to 6.55x, via `format=rtsa`.**
  Captured from RTSA-Suite's own HTTP Client block:
  `GET /stream?format=rtsa&rate_reduction=8&input=main&compression=5&rate_adaption=0`.
  `format=rtsa` streams the file container and is the only format that
  accepts `compression=N`, which applies the file format's own lossy
  codec. The container carries `float32` (`mSampleType` 11, `DSST_F32N`),
  so level 0 is plain float32 with under 1% of chunk overhead — 123.0
  MB/s against 122.9 theoretical at 15.36 MS/s. Against that baseline the
  codec buys 2.74x at level 1, 4.43x at level 5 and 13.10x at level 9;
  even level 1 undercuts plain `int16` while carrying float precision.
  Ratios hold at 3.84 MS/s too.

  HTTPSPEC had documented `format=rtsa` only as the thing a typo falls
  back to — "a completely different wire format rather than an error" —
  and this release had gone on to claim compression was neither offered
  nor useful. Both are corrected. Generic HTTP compression is still not
  available on `/stream` and still would not help (zlib manages 1.07x on
  `float32`); Aaronia's codec wins by being lossy and signal-aware.

  This crate cannot use it for IQ, tested rather than assumed: a real
  compressed payload handed to `Decompressor::decompress` comes back
  rejected as proprietary `DSPT_IQ`, the same wall that stops compressed
  IQ *files*. `format=rtsa` also defaults to `mCompression=1`, so only
  `compression=0` is decodable and that is 20% larger than plain `int16`.
  Spectra should decode, `DSPT_SPECTRA` being documented, but this
  mission has no spectra input to try.

### Performance
- **The control plane is now requested compressed.** `reqwest` gains the
  `deflate` and `gzip` features, so the client sends
  `Accept-Encoding: gzip,deflate` where it previously sent none. Measured
  against the device: `/remoteconfig` 17,432 bytes to 3,299 deflated,
  `/healthstatus` 6,100 to 1,436. Both are read at every device open, and
  `/healthstatus` again on each stream-gap report. No effect on `/stream`,
  which the server does not compress at any level.
- The reader channel's size comment claimed ~157 KiB chunks and ~10 MB of
  queue. Measured against a live server it is 64 KiB for 71% of chunks,
  so the queue is ~4 MB, about 45 ms at the 88 MB/s a WiFi 6E path
  delivers.

## [v0.8.1] - 2026-09-07

### Fixed
- **SoapySDR TX gate read the arguments, not the backend.** It also
  required a compile-time feature no CMake build passed, so no shipped
  plugin could ever report TX. The sink is now built when the constructed
  source's `source_type` is `NativeSdk` — which covers a bare
  `driver=aaronia` that auto-detects the SDK, and correctly refuses when
  a `serial=` open fell back to HTTP because the SDK is not installed.
  New `-DAARONIA_NATIVE_SDK=ON` builds the Rust library with the feature.
- **`hasHardwareTime("")` returned true on the file and native-SDK
  backends**, which never set a timestamp — the 1970 bug the previous
  fix removed from HTTP, relocated. Gated on the backend now.
- **`getAntenna` still hardcoded `RX1`/`TX1`** while `listAntennas` had
  become device-derived, so a V6 in an RX2 mode reported an antenna not
  in its own list. Both now agree; `getClockSource` likewise always
  returns a member of `listClockSources`, and `setClockSource` re-reads
  the device before deciding a request is a no-op instead of comparing
  against a construction-time cache.
- **A trailing slash on the base URL produced `//info`, which the RTSA
  server answers with 404** — every control-plane request failed.
  Stripped in `HttpEndpointsClient::new`.
- **`DeviceHealthSummary::losing_nothing` used exact float equality** on
  counters that decay: a stale 2e-7 errors/s blamed the device on every
  gap report. A 0.5/s threshold now.
- **Device identity was read from the first `info` group in the tree**,
  which on a mission listing the HTTP Server block first reported
  `hardware=HTTP Server`. Lookups anchor to the block owning
  `centerfreq0` / `devstate`.
- **A declared `0..0` range was published as a capability**, leaving a
  GUI that clamps to it unable to tune. Ranges must be finite with
  `min < max`.
- **`enum_options` kept empty entries** (a trailing comma became a blank
  clock source); it filters them, and `enum_values_of` is gone.
- **The sample-rate ladder was capped at 10 rungs** whatever the device
  declared; it is as deep as `decimation0` says
  (`utils::iq_ladder_from_top_n`). `getSampleRateRange` now returns one
  zero-width range per rung instead of a continuous span.
- **`make clean` in the plugin build deleted the shared Rust archive**
  via `BYPRODUCTS`; removed.
- **Overlapping stream-gap health probes could overwrite a newer reading
  with an older one.** Readings carry the drop count they were taken at
  (`StreamStats::device_health_drops`) and never regress.
- **SDK detection only looked in `sdk/`**, so RTSA-Suite 3.0.3 for Linux
  — which ships `libAaroniaRTSAAPI.so` in the install root — was reported
  absent. Both layouts are checked, and an `AARONIA_SDK_PATH` pointing at
  `sdk/` resolves via its parent. Health walker accepts `gpssats`; the
  receive path logs packet warning flags at debug.

### Added
- **`scripts/sdk-container-test.sh`** loads the real SDK library in an
  x86-64 Linux container and runs the crate's native-SDK load test
  against it — no hardware and no x86-64 host needed. Verified against
  3.0.3.16655: `AARTSAAPI_Version()` reports 1.4 and the default Linux
  install path is detected with no environment variable. The nine host
  packages the bundled Qt needs are recorded in SDKSPEC.

### Performance
- **No per-read allocation on the hot paths.** The C ABI and seify reads
  allocated a fresh Vec per call — 512 KiB malloc/free hundreds of times
  a second at full rate; they borrow a scratch buffer held by the source.
  The Python dual-channel read pre-sizes its two vectors.
- `get_device_capabilities` issues its two GETs concurrently, halving
  `Device::make`'s cost and its worst case.

### Documentation
- README and PLUGINS pinned `"0.7"` after the breaking 0.8.0; now `"0.8"`.
  PLUGINS.md's TX section matches the new condition. SDKSPEC's Verified
  Architecture and Qt-dependencies sections carry the 3.0.3.16655
  findings. Two dead CMake debug probes (`test.cmake`,
  `soapy-aaronia/print.cmake`) are gone.

## [v0.8.0] - 2026-09-07

**Breaking:** `StreamStats` gained a `device_health` field, so exhaustive
struct literals over it stop compiling. It is `#[non_exhaustive]` now, with
the new `DeviceCapabilities` and `DeviceHealthSummary`. Only the `futuresdr`
feature exposes `StreamStats`.

### Added
- **The SoapySDR probe reports the attached device, not compiled-in
  constants.** It reads `/remoteconfig` and `/healthstatus` once at open and
  publishes model, serial, firmware version, frequency and gain ranges with
  their steps, clock sources, antenna and the sample-rate ladder. A V6 ECO
  declares 5.5 MHz–8 GHz and −55…+23 dBm; the constants said 10 Hz–6 GHz and
  −100…+10 dB.

  Sample rates come from `status/iqsamples` snapped to an exact
  `receiver_clock / 1.5` rung — new `utils::snap_to_ladder_top`, because the
  reported 61 411 246 Hz is a measurement of a nominal 61 440 000 — then
  halved over `decimation0`'s rungs. `getSampleRateRange` takes its ends
  from that ladder; its old 10 kHz floor sat below the slowest rung.

  New API: `DeviceCapabilities`, `ValueRange`,
  `HttpEndpointsClient::get_device_capabilities`,
  `AaroniaSource::device_capabilities`, and the C entry points
  `aaronia_source_get_capabilities` / `aaronia_source_capabilities_free`.
  Fields are independently optional; a backend that cannot answer falls back
  per field rather than wholesale.
- **A stream gap says whether the device caused it.** `HttpSource` reads the
  device's own per-second loss counters (`status/errors`, `usboverflows`,
  `dsboverflows`) on each gap report. Nonzero: the loss starts at the
  device. Zero: it went missing downstream, in the server's 8 MB outbound
  buffer or on the wire, where a second client is one cause. The read is
  detached — the control-plane timeout is 30 s, and stalling `work()` would
  cause the loss it diagnoses. Also `get_device_health()` and
  `StreamStats::device_health`.

### Fixed
- **Clock source reported as `Internal`, a name the device does not use, on
  hardware locked to an external 10 MHz reference.** `device/sclksource`
  offers `Consumer`, `Oscillator`, `GPS`, `PPS`, `10MHz` and three
  `… Provider` variants. `listClockSources` and `getClockSource` now answer
  from the device; `setClockSource` accepts the current source and warns
  otherwise. `listAntennas` takes its name from `device/devicemode`.
- **A TX channel was advertised on every device, including receivers that
  cannot transmit.** `aaronia_sink_build` allocates unconditionally, so the
  existing `_sink ? 1 : 0` guard was never false and an application failed
  on the first write. TX now needs the new `aaronia_sink_supported()` and a
  `serial=` open with no `url=`/`file=`. The probe reads `0 Tx`. Still not a
  hardware check: a native-SDK build opened by serial against an ECO would
  advertise TX.
- **`--probe` reported `Timestamps: NO` on a device that timestamps every
  buffer.** `hasHardwareTime("")` returned the last timestamp, which is 0
  until a packet arrives — and a probe never streams. It is a capability
  query now. `hasHardwareTime("GPS")` still reports a value, since a fix may
  not exist.
- **The plugin could link a stale Rust archive.** The CMake rule used
  `add_custom_command(OUTPUT …)` with no `DEPENDS`, so cargo was skipped
  whenever the `.a` existed and source edits silently never reached the
  module. It is a custom target now, run every build.
- **Three clippy lints left CI red since v0.7.7** — a collapsible `if let`,
  a needless borrow and a `field_reassign_with_default`, all in code gated
  to Windows and Linux, which a macOS `cargo clippy` never compiles.

### Documentation
- **Several clients on one HTTP Server block, measured.** The block serves
  each connection a full copy and refuses none, so *n* clients cost *n*
  times the egress. Two at 15.36 MS/s ran contiguous; five saturated 2.5GbE
  at 293.9 MB/s and the loss fell on an arbitrary two, a different pair on a
  repeat run. No endpoint counts connections — `/info`, `/healthstatus`,
  `/remoteconfig` and the `/stream` headers were all checked. So a clean
  stream is not evidence of being alone, and a gap is not evidence of
  company.
- **Corrected HTTPSPEC's free-licence claim.** Five clients were served on a
  system holding one HTTP Server block licence: the limit is on block
  instances, not connections to one block.
- **Link requirements, measured over 2.5GbE.** A 2.5GbE table in
  `link_budget` and a requirements section in the README. An ECO 100 needs
  the 61.44 MS/s rung at 245.8 MB/s, past gigabit. The path saturates at
  292 MB/s, 93 % of line rate, so the wire is the limit and not the server;
  and two configurations asking 245.8 MB/s by different routes both
  delivered 244 MB/s, confirming only the byte rate matters.

## [v0.7.7] - 2026-09-06

### Added
- **`link_budget`, so a span the path cannot carry is caught before the
  capture instead of after it.** `required_byte_rate` gives what a span
  costs, `max_sustainable_span` the widest rung a measured rate affords, and
  `measure_link_throughput` measures the path end to end off `/stream` — end
  to end because the bottleneck may be the server, a switch, the air or this
  host's ingest, and the NIC's link speed sees none of them. Measured over
  gigabit: `--span 10M` (15.36 MS/s, 61.4 MB/s) ran contiguous; `--span 20M`
  (30.72 MS/s, 122.9 MB/s) lost 1.84 s of 35 s across 1024 skips.

  The probe discards a 500 ms settle window (`LINK_PROBE_SETTLE`) first.
  Without it a probe *lies*: the server hands over ~0.27–0.35 s of
  pre-connect backlog faster than real time, so counting it reads above the
  true rate and waves through a span that cannot fit. An unreachable server,
  an idle mission or a stream that stops mid-window is an error and never a
  rate, since "0 MB/s" would condemn every span on the ladder.
- **`HttpSource` runs the same check passively and warns once**, on the
  stream it is already reading — no second connection. It names the span,
  its requirement, what was measured and the widest span that would have
  fitted. Complements `DropDetector`; when both fire, the gap warning cites
  the verdict's figures instead of reading as an unrelated second fault.
- **`utils::IQ_RATE_CLOCK_RATIO`, `utils::iq_ladder_from_top`,
  `StreamFormat::iq_bytes_per_sample` and `PacketMetadata::sample_rate()`** —
  names for constants and rules that had been duplicated literals.

### Fixed

The link-budget check, all within this release:
- Judges against the device-reported rate, not the requested one.
  `current_sample_rate`'s 10 % hysteresis meant the builder's 1 MS/s default
  (served by the 0.96 MS/s rung, a 4 % gap against a 2 % tolerance) warned on
  every healthy start. A mid-measurement external retune restarts the meter.
- Counts decoded IQ payload once per sweep rather than raw chunk bytes.
  Per-chunk counting stamped a whole drained batch at one instant, inflating
  the first post-settle sweep; raw bytes also counted JSON headers and any
  spectra sharing the stream.
- A consumer stall no longer dilutes the window — an 8 s stall used to
  average dead time into the sustained rate. The observation crossing the
  boundary is excluded in both directions, and `finish` refuses to answer
  before the window closes or when it observed under half its span.
- A configuration restart re-arms the check; reconnects keep the verdict. A
  parse error in the closing sweep no longer discards a finished measurement.
- The remedy halves down from the device's own rate, so it names a rung the
  device has, and takes its figures from the stream format instead of
  hardcoding int16's. float32 streams are told int16 halves the requirement.
- Rate tracking and the meter key on payload type. A spectra header carries
  no `sampleFrequency`, so it yielded a frame rate that ping-ponged the
  tracker, and its scalars were counted at IQ byte width.
- A failed measurement re-arms instead of retiring permanently; the
  no-verdict state is reserved for configurations that cannot be measured.
- `measure_link_throughput` refuses windows under `MIN_PROBE_WINDOW`
  (100 ms) and takes the capture's own `StreamParams` and settle window.

Streaming and parsing:
- `rate_reduction(0)` is refused with `Error::Config` at all three entry
  points (new `StreamParams::validate`); it used to reach the server.
- One framing implementation, `scan_packet_header`, shared by the parser and
  the probe's sniff. They had disagreed on resync, and the old scan was
  quadratic in brace-dense garbage — 256 KiB of `{` took 20 s of a tokio
  worker.
- An HTTP source starts again after `stop_streaming`; a SoapySDR
  deactivate/activate cycle used to fail.
- An undecodable packet is skipped instead of wedging the stream and growing
  the buffer without bound. `HttpSource` resets parser and drop detector on
  every reconnect.
- `read_samples` returns a partial block on timeout instead of losing it — an
  unflagged gap through the C and Python APIs.
- `IqPacket.sample_rate_hz` reports the streamed rate, not the requested one.
- The shared client builder no longer pins HTTP/1.1, so ALPN negotiates over
  `https://`, with HTTP/2 adaptive flow control — hyper's default 2 MiB
  window would otherwise have been measured and blamed on the path.

RTSA files, native SDK and API surface:
- `mSampleSize` counts values per sample, not bytes. Used as a byte stride,
  a second partial chunk read landed mid-sample and spectra returned one
  scalar per spectrum.
- A negative DSFT stream offset no longer overflows the header search;
  `seek_to_sample` works on reverse-order files; spectrum decompression caps
  its output size from packet metadata.
- The native SDK is shut down by the last client, not the first —
  `AARTSAAPI_Shutdown` is process-wide, and each source and sink called it
  from its own `Drop`.
- A bare `spectranv6eco` opens `spectranv6eco/rtsa`; the ECO has no `/raw`.
- Native SDK: `aaronia_source_read_samples_timeout` honours its deadline (it
  polled up to 500 ms regardless); spectra reads honour the packet stride; an
  SDK call returning no object is an error, not a null pointer; a second
  device open on a live source is refused; the device closes on drop.
- The remote-config licence probe writes the discovered block, so it finds
  `reflevel0` in a V6's receiver block and can report `Active` — it always
  answered `NotLicensed`.
- `HttpSource` built outside a Tokio runtime errors at build rather than
  panicking inside `init()`.
- `aaronia_get_error_message` takes an `int` and answers unknown codes, where
  an out-of-range enum was undefined behaviour. C callers are unaffected; a
  Rust caller passes `code as i32`.
- `HttpSink` clamps `buffer_size(0)` to one; a quiet dwell in hop mode is an
  empty read, not an error counted toward the source-dead bailout; the first
  packet after a failed initial connection is not flagged as an overrun;
  seify reports the observed rate over the ladder's 120 kS/s floor.
- Clippy passes on Rust 1.98 (`chunks_exact_to_as_chunks`, eight sites).

### Changed
- **Byte-rate helpers answer `Option<f64>`, never a `0.0` sentinel** —
  `required <= measured` on a garbage rate read "fits", a convention every
  comparison site had to remember. `StreamFormat::iq_bytes_per_sample` is
  `Option<usize>` (`None` for JSON) for the same reason. New
  `max_sustainable_sample_rate_below` anchors a remedy to the device's rate.
- **The verdict is data, not just a log line.** `LinkBudgetVerdict` is
  published as `StreamStats::link_budget`, so a GUI or orchestrator can
  auto-narrow a span without scraping logs, and `LinkBudgetVerdict::judge` is
  its one producer, so the struct's invariants hold by construction.
  `StreamFormat::CAPTURE_DEFAULT` replaces three separately written `Int16`
  literals.
- **One copy each of the RTSA client plumbing.** Construction, auth,
  base-URL validation, `/stream` query serialization and status-to-error
  mapping had hand-rolled duplicates in the probe — five drift risks, and the
  three reqwest clients had already drifted to three setting subsets.
- **`HttpSource`'s stream-open failure is `Error::Http { status, context }`**,
  the variant the rest of the crate returns. Code matching the old
  `Error::Protocol("Stream endpoint returned error: …")` text needs updating.
- `work()`'s output copy is two bulk `copy_from_slice` calls over the deque's
  contiguous halves instead of a `pop_front` per sample.

### Performance
- The parser drops an over-cap payload as it arrives instead of buffering it;
  uncompressed spectra decode without a copy; spectra file reads decode in
  place; the C API reserves the caller's length before a read;
  `sample_rate_hz` is an atomic load, so stamping every packet takes no lock.

### Documentation
- **The HTTP transport measured on WiFi 7.** `curl` on `/stream` sustains
  ~75 MB/s (0.6 Gbit/s) station to station at a 2.4 Gbps PHY — both ends on
  air halves the medium — and the figure is identical for all three wire
  formats, so the encoding is not the limit. Two parallel connections
  aggregate *less* (63.9 against 74 MB/s), so one connection is optimal.
  Against that, the crate's framing-plus-decode path measures ~3 GB/s and the
  individual decoders 0.8–10 GS/s: fortyfold headroom, never the bottleneck.
  At 4 bytes a sample the link buys ~19 MS/s — the 15.36 rung fits, 30.72
  does not, and full span needs a wired path. Two ignored throughput-meter
  tests keep the numbers re-measurable in one command.
- README and PLUGINS pinned `sdr-aaronia-rs = "0.6"`, a major behind the
  crate, so nothing they described resolved. Now `"0.7"`.
- `cumulative_drops` counts client-detected timestamp gaps; four docs called
  them server-reported drops. Bandwidth figures use the 61.44 MS/s top rate
  rather than the 92 MHz clock.

## [v0.7.6] - 2026-08-15

All of this is the FutureSDR `HttpSource` block (the `futuresdr` feature),
whose streaming path turned out to be losing most of the stream. Found and
measured against a live SPECTRAN V6 ECO running a 49 MHz survey.

### Added
- `iq_sample_rate_for_decimation_index`, `decimation_index_for_rate` and
  `decimation_index_for_bandwidth` in `utils`: the RTSA "Span" enum (`Full`
  … `1 / 512`) mapped onto the sample-rate ladder and back, so a requested
  span becomes the index the device takes.
- `HttpEndpointsClient::apply_capture_config` — retune via `/remoteconfig`
  with read-back confirmation — plus `find_block_name_with_field`, which
  discovers the receiver block by the field the write targets rather than
  assuming its category.

### Fixed
- **The sample buffer guillotined every packet bigger than itself.** With
  capacity fixed at `buffer_size * 2`, a device sending 49k-sample packets
  into a 16384-sample capacity lost ~75 % of every packet before the
  consumer saw any of it. Measured live: 61.4 MS/s at the device, 0.33 MS/s
  reaching the pipeline, digital decode unable to hold frame sync. Capacity
  now floors at four times the largest packet observed.
- **The initial tune could claim success while the device never moved.**
  `/control` answers `success=true` whether or not a block applies the
  command. The start-up tune now writes `centerfreq0`, `decimation0` and
  `reflevel0` via `/remoteconfig` to the block found by walking the config
  tree, reads them back and warns about anything that did not take. Where no
  such block exists it falls back to `/control` and says the result is
  unverified. The tune is one-shot, so a restart after an external retune no
  longer re-pushes a stale target.
- **Launching an app no longer overwrites the operator's gain.** The
  builder's reference level was pushed on every start; it is optional now
  and left untouched unless set.
- **`work()` spun.** The runtime ran it whenever the output port had any
  room — 106,000–390,000 calls a second averaging 14 free samples each,
  burning ~60 % of a core and starving the downstream block. The port now
  requires a worthwhile block of room; call rate dropped to ~14/s.
- The refill loop kept iterating through its 50 ms idle sleeps when the
  channel was empty, holding buffered samples for up to 800 ms. It flushes
  immediately and sleeps only with nothing to flush.
- A parse error mid-stream reconnected without reaping the reader task,
  which on a stalled socket holds a server connection open.

### Changed
- **The HTTP socket is drained by a dedicated task**, not inside `work()`.
  Reading it only when the scheduler ran the block capped throughput at a
  tenth of what `curl` pulls from the same endpoint. A background task now
  pushes chunks into a bounded channel (~10 MB) whose fill is the
  backpressure point: consumer falls behind, channel fills, reader blocks,
  TCP flow control stops the server. Measured at 49 MHz span: 1.8 → ~7–9
  MS/s. The ceiling is the link (~57 MB/s ≈ 14 MS/s as float32).
- Buffer-overflow drops are counted and logged geometrically rather than per
  occurrence — 5040 log lines in 25 seconds at wide span, for one fact.
- File playback decodes little-endian cf32 straight into the sample buffer;
  the per-sample decode remains for big-endian hosts, with a round-trip test
  pinning the two to identical output.

## [v0.7.5] - 2026-08-13

### Added
- **`scale` on the Python config and `aaronia.open()`**, the integer encode
  multiplier for the `I16` wire format. Rust, the C API and the SoapySDR
  plugin all had it; Python did not, leaving no way out of the trap below.
- **`scripts/validate-iq-live.py`**, an end-to-end check that the samples an
  application receives are the ones the device sent. Every wire format must
  decode to the same spectrum, the Python and SoapySDR paths must agree, and
  a known transmitter must land where it should. Against a live server it
  places a NOAA carrier within 312 Hz of 162.400 MHz and on the correct side
  of zero — the one check that catches transposed I and Q.

### Fixed
- **`format="I16"` silently discards weak signals at the default scale.**
  The server sends `round(value * scale)`, so the step is `1 / scale`, and
  the default 16384 gives 6.1e-5 — coarser than a quiet band's noise floor.
  Measured: **68 % of int16 samples came back exactly zero** where float32
  had none. At `scale=1e6` the zero fraction was 0.0 % and the amplitude
  matched float32.

### Changed
- **The Homebrew formula is no longer a release asset**, but the
  `homebrew-formula` workflow artifact. Homebrew 4 cannot install from a
  formula URL, so on the release page it was a file no user could act on.
  Checksums are still rendered against the published archives.

### Documentation
- **Wire format is a throughput decision, and the default is the expensive
  one.** At 15.36 MS/s over a LAN float32 needs 123 MB/s: measured, it
  delivered 6.5 MS/s with 290 drops, while float16 and int16 both delivered
  15.1 MS/s. The Python README carries the numbers.
- **What the 80 % usable-bandwidth figure is.** RTSA declares exactly
  0.8 × Fs as the packet's frequency range at every rate — checked at 61.44,
  15.36, 7.68 and 3.84 MHz — and every sample still arrives, so an FFT spans
  the whole rate while only that 80 % is flat and calibrated. Sweeping a V6
  ECO's own noise floor confirms it: flat within 0.5 dB across 0.80 of the
  rate at 15.36 MHz and 0.89 at 7.68 MHz, and at full span the analog filter
  is ~1 dB down by the declared edge — which is where Aaronia's 44 MHz
  data-sheet figure comes from, against the 49.152 MHz the device declares.
  To see N Hz of spectrum, sample at N / 0.8.
- **The READMEs stated the V6 ECO's ladder as if it were every device's.**
  61.44 MHz halved to 120 kHz is measured and true for an ECO; a full V6
  selects its receiver clock and starts higher. The Python, SoapySDR,
  quickstart and applications docs now say whose ladder it is, as does
  `iq_sample_rates`. The SoapySDR README gained a Sample rates section.
- The same sweep caught universal-sounding claims that are one device's
  measurements, in `unified_source`, HTTPSPEC's `/control` span note and the
  0.8 ratio. `seify_impl`'s range is capped at 61.44 MHz for the same reason
  and says so: seify has no device handle there to ask for better.
- SDKSPEC gave the eco family's clock as 61.44 MHz in a second place. It is
  92.16 MHz; 61.44 MHz is the top IQ rate.

## [v0.7.4] - 2026-08-12

### Fixed
- **The SoapySDR plugin ignored an unrecognised `format=` silently.** A
  device string carrying `format=int16` — the wire name rather than the
  plugin's `I16` — streamed the default format while claiming otherwise. It
  warns and continues now. The server behaves worse: an unrecognised
  `format=` on `/stream` serves the RTSA file format with HTTP 200 rather
  than an error, so a typo changes the wire format entirely. `raw16`, which
  Aaronia's Qt reference client sends, is a working alias for `int16`.

### Documentation
- Checked Aaronia's V6 remote control notes (rev 4, May 2026) against the
  hardware. `/remoteconfig` enum fields take an index as well as a label;
  one `simpleconfig` PUT can carry several groups, and groups other than
  `main` work; a PUT naming a block not in the mission returns 200 and
  changes nothing, which `simple_remote_config` now warns about since it
  reported `Ok(())` for a write that did not happen. In the config-tree form
  the receiver name is ignored — the write is routed by `config.name`.
- Documented loading a mission over `/control`, and that every `/control`
  payload needs its `type` or the server answers `400`. Loading a mission is
  deliberately not exposed: swapping the mission under a running capture
  should be a caller's decision, not a side effect.
- RTSA-Suite has no status endpoint; Aaronia's own liveness check reads the
  `404` from `/api/status` as proof the server is up.
- **What "Full" means on a full V6 is unresolved.** A V6 ECO follows the
  SDK's `spanfreq <= receiverclock / 1.5`, measured. Aaronia's Remote Config
  screenshots show a full V6 at a 92 MHz clock delivering 92.16 MHz of IQ
  samples per second at span "Full" — the clock itself.
  `iq_sample_rates_for_clock` may therefore understate the top of the ladder
  by 1.5× for a full V6 at a non-default clock; it says so now.
- HTTPSPEC contradicted itself on the Remote Config licence. A live
  unlicensed system accepts writes, re-confirmed for centre frequency,
  decimation, reference level and the preamplifier.
- SDKSPEC gave the V6 ECO's receiver clock as 61.44 MHz, which 0.6.2
  corrected in code to 92.16 MHz. 61.44 MHz is the ECO's top IQ rate.
- **A marker stream is not a categories packet**, which the previous draft
  of this entry got wrong. Aaronia's example declares `payload: "spectra"`,
  and spectra samples are a 2D array, so its nesting is correct. Its three
  frequency fields are all zero, so the category names and ranges are the
  only description of what the numbers mean.
- Folded in Aaronia's endpoint specification (rev 11). `/control` takes PUT
  only, and a command reaches every block that understands it unless
  `receiverUUID` or `receiverName` scopes it. Per-type settings are listed
  in full, including `deviceconnect` and `camera`, which this crate does not
  model; zones cannot be configured remotely. The server drops data once its
  outbound TCP buffer passes 8 MB, the mechanism behind most unexplained
  gaps. `/healthstatus` is organised as `info`, `status`, `health`,
  `settings` and `components`.
- **`status/iqsamples` is the native rate, not the delivered one.** It held
  at 61.44 MHz while the same device delivered 15.36, then 7.68, then
  61.44 MS/s. Read `sampleFrequency` from packet metadata instead.
- One HTTP Server and one HTTP Client instance are free; additional
  instances are licensed separately, as are Stream Merger and Stream
  Splitter. **This entry originally read that a second concurrent client
  meets the limit — corrected in v0.8.0**, where five simultaneous clients
  were served on a one-block licence. The limit is on block instances in the
  mission graph, not connections to one block.

## [v0.7.3] - 2026-08-12

### Fixed
- The Windows leg of the new module load check could not run: vcpkg's
  SoapySDR port ships no `SoapySDRUtil`, so the check failed rather
  than verifying anything, and 0.7.2 published no release archives.
  Where the tool is absent the packaged DLL is now loaded directly,
  which still catches a module whose dependencies do not resolve away
  from the build machine.

## [v0.7.2] - 2026-08-12

### Fixed
- **The published macOS SoapySDR module would not load, on any Mac.**
  `dlopen` failed with "symbol not found in flat namespace" for a Rust
  vtable entry that the linker had itself defined and localised in the
  same image. SoapySDR's installed CMake export lists `-flat_namespace`
  in the imported target's interface, so it reached the end of every
  module's link line and forced flat-namespace binding; the module
  links directly against libSoapySDR and does not need it. Removing it
  takes the module from 5670 flat-namespace binds to none, so the
  failure cannot recur rather than depending on a linker version — some
  hit the defect and some did not, which is why local builds worked.
  Every 0.5.x, 0.6.x, 0.7.0 and 0.7.1 macOS archive is affected; the
  Linux and Windows modules are not.
- **The packaged module's load check ran on Linux only**, which is how
  the above shipped for as long as it did. It now runs on all three
  platforms at release time, and CI builds and checks the plugin on
  macOS as well as Linux. Both check the reported text: `--check` exits
  0 even when the driver failed to load.

### Documentation
- The Linux module needs glibc 2.38 or later, so it does not load on
  Ubuntu 22.04 or Debian 12. Said so in the plugin README.

## [v0.7.1] - 2026-08-12

An incomplete fix for the macOS module, superseded by 0.7.2. Like
0.7.2, it reached crates.io and PyPI but published no release archives,
because the new load check refused to ship a macOS module that would
not open. Its crate and wheels are sound and identical in content to
0.7.3.

## [v0.7.0] - 2026-08-12

### Added
- **`aaronia.open()`, block iteration and context-manager support in
  Python.** The shortest working program is now three lines. `open()`
  takes the URL, frequency and either an exact `rate` or the
  `bandwidth` you want covered, connects, and starts streaming;
  `for block in src.blocks(65536)` ends when the source runs out
  instead of raising; and `with` stops the stream even when the body
  fails, raising a failed teardown only if the body itself succeeded.
  The old config-object path is unchanged and still the way to reach
  every option.
- **`aaronia-doctor`, a command that checks an RTSA server.** It reports
  whether the server answers, whether the mission has an input carrying
  IQ, and what rate the device is running, and prints the fix for each
  failure. The same checks are available as `aaronia.diagnose(url)`,
  which returns `(ok, message, fix)` tuples, bounded at 20 seconds so a
  stalled server cannot leave it waiting. Every failure it names is one
  that otherwise shows up as a timeout with no explanation.
- **`aaronia.sample_rates()` and `aaronia.sample_rate_for_bandwidth()`**,
  exposing the crate's rate ladder to Python so a program can ask for a
  rate the hardware will actually run.
- **`Error::StreamClosed`**, separating "the stream ended" from "a read
  failed". Both used to arrive as `Error::Protocol`, so a consumer
  could not tell a capture that finished from one that was cut short.
  Rust code matching on `Error::Protocol` for the closed-stream case
  needs the new variant instead; the enum is `#[non_exhaustive]`, so
  existing wildcard arms keep compiling. In Python the matching
  exception is `AaroniaStreamClosed`, a subclass of
  `AaroniaConnectionError`, so existing handlers are unaffected. It is
  what lets `blocks()` end a loop on a finished stream while still
  raising on a timeout or a transport failure, which would otherwise
  make a truncated capture look like one that simply ran out.
- **An installer in every SoapySDR release archive.** `install.sh`
  (`install.ps1` on Windows) finds SoapySDR's module directory, clears
  the macOS quarantine flag, copies the module in, and confirms it
  loads. It prints instructions rather than guessing when SoapySDR is
  missing.
- **A Homebrew formula for the SoapySDR plugin**, in
  `packaging/homebrew`. The release workflow renders it against the
  published archives, checksums included, and attaches it to the
  release, so updating a tap is a copy.

### Fixed
- The SoapySDR application guide still listed the old invented sample
  rates for GQRX. It now describes the real ladder.

## [v0.6.2] - 2026-08-12

## [v0.6.1] - 2026-08-12

### Fixed
- **The SoapySDR plugin advertised sample rates the hardware cannot
  produce.** Seven of the ten rates it listed, 1, 2, 5, 10 and 20 MHz
  among them, do not exist on the device, which runs at 61.44 MHz
  divided by a power of two. Applications build their rate dropdowns
  from that list, so choosing 10 MHz ran the device at a different rate
  while the application went on displaying 10. The list is now the real
  ladder, 61.44 MHz down to 120 kHz, and `setSampleRate` snaps to the
  nearest one and logs when it has to.
- **The crate reported the requested sample rate rather than the one in
  use.** The device adjusts a rate it cannot produce, so
  `get_source_info()` described a capture that was not happening. HTTP
  sources now report the rate, centre frequency and usable bandwidth
  from the stream's own metadata once packets arrive.

### Added
- `iq_sample_rates`, `usable_bandwidth_hz`, `nearest_iq_sample_rate` and
  `iq_sample_rate_for_bandwidth` in `utils`, with the constants
  `IQ_CLOCK_HZ` and `USABLE_BANDWIDTH_RATIO`. The device samples at
  61.44 MHz divided by a power of two and delivers 0.8 of that as
  alias-free bandwidth. Callers were deriving their own rates from
  guesses, so the relationships now live in one tested place. Wanting
  8 MHz of spectrum needs 10 MHz of sampling, and
  `iq_sample_rate_for_bandwidth` returns the 15.36 MHz that provides it.
  `iq_sample_rates_for_clock` covers devices whose receiver clock is not
  the default: a V6 ECO has a fixed clock and gives the measured ladder,
  while a full V6 can select a faster one and reach further. Aaronia's
  samples set `device/receiverclock` to "92MHz" or "245MHz"; only the
  default has been checked against hardware.

### Added (native SDK)
- **Device-family auto-detection.** `detect_device_family` and
  `open_detected_device` try each known family in turn, so an ECO owner
  no longer has to know that the default `spectranv6` will not find
  their device and that `spectranv6eco` is the string they needed.
- **`read_spectra`**, with the stream index taken from the open mode
  rather than assumed. `spectranv6/raw` carries spectra on stream 2 and
  IQ on stream 0; every other mode uses stream 0. Hardware-unverified.
- **`receiver_clock_hz`** on the native source, and
  `spectranv6eco/rtsa` added to the known open modes. The clock sets the
  rate ladder's ceiling, so callers that need to know which rates exist
  can now ask instead of assuming.

### Fixed (native SDK)
- **The V6 ECO's fixed receiver clock was recorded as 61.44 MHz.** It is
  92.16 MHz: an ECO streams at 61.44 MHz sampling, measured against real
  hardware, and the constraint checked at configuration time is
  `span * 1.5 <= clock`. The old value rejected every span above
  40.96 MHz, including the device's own maximum.
- **Dual-channel capture selected the wrong mode and would have
  returned corrupted samples.** `RxChannel::Rx1And2` wrote
  `device/receiverchannel = "Rx1+Rx2"`, which delivers the two inputs as
  two independent streams at indices 0 and 1. This crate reads a single
  stream and deinterleaves it, which is the contract of the other mode,
  `"Rx12"`. On a two-input V6 the result would have been Rx1's samples
  split into two bogus channels, with no error anywhere. It now writes
  `"Rx12"`. Aaronia's `RawIQ2RX` and `RawIQ2RXInterleave` samples
  demonstrate one mode each. Still hardware-unverified.
- **Sweep mode set the wrong resolution-bandwidth key.** It sent
  `main/rbw`, which no Aaronia sample uses; the key is `main/rbwfreq`.
  Checked against Aaronia's published `SweepSpectrumEco` sample, which
  also confirms `main/startfreq`, `main/stopfreq`, `main/reflevel` and
  the `spectranv6eco/sweepsa` open string that this crate already used.
  The sweep path remains hardware-unverified.

### Documentation
- **GPS time needs GPS switched on, and the crate does not do it.**
  Devices ship with `device/gpsmode` disabled, so `get_gps_time` would
  return `None` indefinitely and appear broken. Aaronia's `GPSTime`
  sample sets `device/gpsmode` to "Location and Time" and
  `device/sclksource` to "GPS Provider" before starting the device;
  `get_gps_time` now says so.
- Documented what `/control`'s `frequencySpan` actually means. It is a
  request for usable bandwidth, not a sample rate: the device picks the
  rate whose alias-free span is nearest, so 2.5 MHz yields 3.84 MHz and
  10 MHz yields 15.36 MHz. Values on the rate ladder round-trip exactly,
  which is why the field looks like a sample rate in ordinary use.
  Verified across nine requests on a V6 ECO.

## [v0.6.0] - 2026-08-11

Reliability and documentation release. The HTTP backend now handles a
server that is still starting up and a stream that drops mid-session.
The documentation now says which features have been tested on real
hardware and which have not.

### Breaking
- `AaroniaConfig` has two new public fields, `read_timeout` and
  `auto_reconnect`. Struct-literal construction needs updating. The
  builder methods are unchanged.
- Dropped HTTP streams now reconnect by default. Previously a dropped
  stream ended the session and every later read failed. Reads can now
  block for up to about 8 seconds while reconnecting. Call
  `auto_reconnect(false)` for the old behaviour.
- SoapySDR plugin downloads are now per-platform archives named
  `SoapyAaronia-<version>-<os>-<arch>.tar.gz`, or `.zip` on Windows.
  The bare `.so`, `.dll` and `.dmg` files are gone. Scripts that
  download them by name need to unpack the archive instead.

### Added
- **Connect retry.** Reaching the server now retries transient failures
  up to 4 times over at most 10 seconds. A `*.local` hostname often
  refuses the first connection while mDNS resolves, which used to look
  like the server was down. Client errors still fail immediately.
- **Automatic stream reconnection** (`auto_reconnect`, on by default).
  The reader reopens the stream up to 5 times, re-applies the current
  tuning, and marks the first packet after the gap as an overrun.
  Re-applying tuning matters because a restarted server returns to its
  mission's frequency, which would otherwise stream the wrong band
  unnoticed. The retry budget resets only after a connection survives
  30 seconds, so a server that accepts and immediately hangs up cannot
  reconnect forever. Also available through
  `aaronia_source_builder_auto_reconnect` and the SoapySDR argument
  `reconnect=0|1`.
- **Configurable read timeout** (`read_timeout`, default 30 seconds),
  replacing a hard-coded value. Available on both builders, as a Python
  property, through `aaronia_source_builder_read_timeout_us`, and as a
  `read_timeout=<seconds>` SoapySDR argument. `read_samples_deadline`,
  which the SoapySDR and seify paths use, still takes its deadline from
  the caller.
- `DropDetector::resync()` clears the packet-timing history but keeps
  the counters. `reset()` clears both, which would make the running
  total jump backwards after a reconnect.
- **Python type stubs.** Editors and type checkers now understand the
  module.

### Fixed
- **Channel hopping could stall for up to 30 seconds.** The hop loop
  waited for a full block of samples, far longer than a 20-40 ms dwell,
  so a slow server starved the remaining hops. It now stops waiting at
  the dwell deadline.

### Documentation
- **The README says what has been tested on hardware.** Each feature is
  marked live-verified, verified manually, mock-tested, or
  hardware-unverified. Transmit, dual-channel and the native-SDK paths
  have never run against a device.
- **New quickstart** (`docs/QUICKSTART.md`) covering RTSA-Suite mission
  setup, which everything depends on and nothing documented: adding the
  HTTP Server block, connecting the device output to it, checking it
  with `curl`, and the mistakes that cost the most time.
- **New application guide** (`docs/APPS.md`) for SDR++, GQRX, GNU Radio
  and SoapySDR from Python. It explains that the single `REF` gain
  element is a reference level in dBm, so raising it reduces
  sensitivity.
- **New usage guide** (`docs/USAGE.md`) holding the worked examples that
  were 60% of the README. They are compiled as doctests now, so they
  cannot fall out of date. The README is down from 516 lines to 291.
- **Install instructions for the prebuilt SoapySDR plugin.** Releases
  always attached built modules, but the README only explained building
  from source.

### Release
- Release archives now carry the module, install instructions and the
  licence, and their names state the version, OS and architecture.
- The Linux module ships stripped of debug symbols, 19.1 MB down to
  15.4 MB. The rest is statically linked Rust, not symbols. The release
  job checks the stripped module still loads before publishing it.
- GitHub releases now use this file's entry for the tag as their
  description, with the generated commit list below it.

## [v0.5.1] - 2026-08-11

### Fixed
- **Retuning silently did nothing on real hardware.** The `/control`
  endpoint only applies a frequency change when `frequencyCenter` and
  `frequencySpan` are both present. Sending one of them returns
  `{"success":true}` and is ignored, so `set_center_frequency` reported
  success while the device kept streaming at its old frequency. This
  affected hop mode and the SoapySDR, seify and Python retune paths. All
  of them now send the full set of values, and `configure_capture` warns
  when given a partial one. Reference-level changes were unaffected;
  they apply on their own.
- Corrected the documents that blamed this on the Aaronia "Remote
  Config" licence. Retuning uses the licence-free `/control` endpoint;
  the licence only gates `/remoteconfig` writes.

### CI
- Clippy now runs across the whole workspace. The `python-aaronia`
  member was never linted and had accumulated 12 errors.

## [v0.5.0] - 2026-08-11

Adds Python bindings, a SoapySDR plugin, transmit support and GPS time,
alongside verification of the RTSA file format against the vendor
specification. Receive paths were tested against real hardware.
Transmit and dual-channel paths were not, and are marked as such in the
documentation.

### Breaking
- `RtsaMetadata` lost `device_name`, `stream_sample_rate` and
  `stream_center_frequency`, and `RtsaSource::stream_info()` returns a
  3-tuple. Those fields came from a file layout no real capture uses and
  were always `None`.
- `SdkConfig` and `AaroniaConfig` gained a public `receiver_channel`
  field, which breaks struct-literal construction.
  `NativeSdkSource::configure_iq_receiver` takes the channel as a fourth
  argument so retunes keep it.

### Added
- **Python bindings** (`python-aaronia`), published to PyPI: abi3
  wheels for CPython 3.9 and later, typed exceptions, live retuning, and
  single-copy reads into NumPy or PyArrow. Blocking calls release the
  GIL, so other Python threads keep running.
- **SoapySDR plugin** (`soapy-aaronia`): receive streaming in CF32 and
  CS16, honoured timeouts with partial reads, safe retuning while
  streaming, and device arguments for URL, file, serial, frequency,
  rate, reference level, wire format and RX channel.
- **Transmit support** through the native SDK, via `UnifiedSink` and the
  `aaronia_sink_*` C API. Hardware-unverified, and unavailable outside
  Windows and Linux.
- **GPS time** through `get_gps_time`, native SDK only.
- **Receiver channel selection** (`Rx1`, `Rx2`, `Rx1And2`) and true
  dual-channel capture through `read_samples_dual`. A read-mode latch
  stops single- and dual-channel reads being mixed by accident.
- `StrmChunk::capture_start_offset`, an undocumented field in RTSA files
  identified as the capture start. Reported time spans now match the
  recorded data.
- `scripts/ci-local.sh`, which runs CI's checks locally, including a
  Linux VM step covering the OS-gated native-SDK code.
- Property tests: opening arbitrary bytes as an RTSA file must never
  panic.

### Fixed
- RTSA chunk parsing corrected against the vendor specification and real
  captures: chunk padding and tail offsets, fixed field sizes, the STRM
  layout, enum numbering, and end-time treated as a duration.
- Compressed spectra chunks now report an error instead of returning
  compressed bytes as if they were samples.
- Native SDK: a corrupt packet no longer blocks every later read, and
  buffers are flushed when streaming stops.

## [v0.3.5] - 2026-08-06

Documentation release. Every Markdown file was checked against the code
and corrected. No API changes.

### Changed
- DESIGN.md, the README, CONTRIBUTING and the three specifications in
  `docs/` corrected against the implementation, with explicit notes
  where only hardware can settle a question.
- Clarified that HTTP retuning uses the licence-free `/control`
  endpoint.
- Examples cleaned up. `http_iq_quickstart` takes the server URL as an
  argument instead of hardcoding a private hostname.
- Five tests that asserted nothing now assert something.

### Fixed
- Stale comments about symbol counts, renamed tests, and inverted
  `?scale=` semantics.

## [v0.3.4] - 2026-07-31

### Fixed
- **Native SDK: the end of every oversized packet was discarded.** The
  reader copied out only what the caller asked for, then released the
  whole packet back to the SDK. The SDK chooses its own packet size, so
  everything past the request was lost. This left holes in the IQ
  stream, which breaks the phase continuity that downstream correlation
  and frequency tracking depend on. Extra samples are now kept and
  returned by later calls.
- **Native SDK: a zero-sized read destroyed a packet.** It now returns
  immediately.
- **Transmit bursts were scheduled at the Unix epoch.** The FutureSDR
  sink set every burst's start time to zero, against the device's own
  master clock. Timestamps now come from the master clock, falling back
  to immediate dispatch when it cannot be read. Hardware-unverified.
- **`http_source` reported a buffer capacity it did not enforce.** With
  a buffer size of zero, a consumer computing a fill ratio divided by
  zero.
- **Diagnostic previews could panic on non-ASCII text.** They cut
  strings at byte offsets, so one accented character in a mission name
  was enough.

### Changed
- `cargo clippy --all-targets` compiles on a default checkout again.
- `SdkConfig::timeout` and `SdkSinkConfig::timeout` documented as
  inert. Nothing reads them. They are kept because they are public.

## [v0.3.3] - 2026-07-31

### Fixed
- **A chunk scan could loop forever** on a file whose signature was
  missing, reachable by opening an untrusted file.
- **A failed configuration probe left the device 1 dB off.** The restore
  now runs on every exit path.

## [v0.3.2] - 2026-07-13

### Added
- `DwellAdvice::channel_override` is honoured, and hop mode no longer
  requires the Remote Config licence.

## [v0.3.1] - 2026-07-11

### Fixed
- **Decompression rejects an out-of-range compression factor** instead
  of overflowing on it. The value can come from a network packet, so a
  corrupt header could previously cause a panic.
- **Decompression truncates over-long coefficient streams**, having
  previously only padded short ones.
- **The capture thread is panic-guarded**, so a panic is logged instead
  of killing the thread silently.
- **HTTP overrun detection reaches `IqPacket::overrun`.** Hop and
  single-channel modes report real overrun status instead of always
  `false`.
- **The C API sample copy is bounds-checked** against the source length
  as well as the caller's capacity.
- Raised the `orecchiette-sdr-source-rs` floor to 0.1.2.

## [v0.3.0] - 2026-07-11

### Removed
- **Breaking: the `file_performance` module is gone**, along with the
  `memmap2` dependency. This memory-mapped reader was never used by the
  crate's actual file path and had no callers. `RtsaSource` covers the
  same needs through buffered I/O.

### Fixed
- A misplaced doc comment marked `with_shared_stats` a no-op when it is
  not.
- `HttpSink` counts a batch as dropped when its sender task has died,
  not only on a failed push. It also stops that task on drop.

### Changed
- The last four native-SDK methods that hand-rolled error checks now use
  the shared path, so failures carry structured errors.
- Shared device-type parsing extracted out of `SdkConfig` and
  `SdkSinkConfig`.

## [v0.2.6] - 2026-07-10

### Added
- **Transmit through the native SDK** (`SdkSink`, `SdkSinkConfig`) with
  FutureSDR integration, matching the receive path.
- `examples/native_sdk_transmit.rs`, sending a LoRa-style chirp.

### Changed
- Replaced the opaque `Error::Sdk(String)` with a structured
  `Error::SdkApi`, so callers can react to specific failures.
- SDK warnings are logged as warnings instead of being treated as fatal,
  matching Aaronia's own drivers.

## [v0.2.4] - 2026-07-10

### Performance
- **HTTP parser:** binary formats read packet headers directly instead
  of building and discarding a JSON document for every packet. Small
  packets parse about 41% faster.
- **Float32 samples** are bulk-copied on little-endian hosts instead of
  decoded one value at a time.
- **File replay** reads a block at a time instead of one sample per
  call.
- Native SDK per-read logging moved to `trace`, off the hot path.

### Fixed
- **Native SDK unsound read path.** A zero-sample request underflowed a
  length calculation and produced an enormous slice. Packet counts and
  strides are now bounds-checked.
- **The HTTP reader task leaked.** It is now stopped when streaming
  stops and on drop, instead of holding the connection open and leaving
  the device streaming.
- **Decompression rejects zero dimensions** instead of looping forever.
- **`MmapRtsaReader::read_chunk` bounds check** no longer wraps on a
  pathological offset.
- **Default reference level corrected** from +20 dBm to -20 dBm. The old
  default desensitised the receiver.
- `HttpSource` reschedules itself after reconnecting.

### Changed
- `HttpSink` honours `timeout_ms`, which was previously ignored.

## [v0.2.3] - 2026-07-05

### Fixed
- **The HTTP source did not tune the device.** It opened the stream
  without sending a control request, so it streamed whatever the
  RTSA-Suite was already set to and ignored the requested frequency and
  span.

### Changed
- `http_iq_quickstart` takes frequency and sample rate arguments and
  logs signal power.

## [v0.2.2] - 2026-07-05

### Changed
- Dropped the explicit minimum Rust version and track stable instead, so
  a dependency raising its own floor cannot break CI.

## [v0.2.1] - 2026-07-05

### Added
- Native SDK receiver channel selection (`Rx1`, `Rx2`, `Rx1And2`).
- HTTP wire format and stream scale settings on `AaroniaConfig` and
  `AaroniaSourceBuilder`.

### Fixed
- Raised the minimum Rust version to 1.86 to fix CI builds. (v0.2.2
  dropped the fixed minimum entirely.)
- Default HTTP format changed from JSON to Float32, fixing crashes at
  high bandwidth.
- Fixed FutureSDR deadlocks in `HttpSink` by moving blocking HTTP calls
  onto a background task.

## [v0.1.1] - 2026-07-03

### Changed
- Disable `futuresdr` on docs.rs.
- Fix the licence badge.

## [v0.1.0] - 2026-07-03

### Added
- Initial release.
