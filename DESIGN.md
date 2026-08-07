# Design: Unified Aaronia SDR Interface (sdr-aaronia-rs)

This document describes the architecture of the `sdr-aaronia-rs` crate, which provides a unified interface to Aaronia Spectran V6 devices across the native SDK, the HTTP streaming API, and offline RTSA files.

## 1. Overview

The Aaronia ecosystem offers three ways to access Spectran hardware: the native RTSA-Suite PRO SDK, the HTTP streaming API, and recorded RTSA files. This crate wraps all three behind a single deterministic facade, `AaroniaSource`, so callers can work with Aaronia hardware without depending on the underlying transport.

## 2. Architecture

The crate is built around an automatic source-detection router that instantiates the correct backend from the provided configuration.

```mermaid
graph TB
    subgraph "sdr-aaronia-rs Unified API"
        API[AaroniaSourceBuilder]
        Detection[Deterministic Auto-Detection]
    end

    subgraph "Backend Transports"
        SDK[Native C++ SDK via FFI]
        HTTP[HTTP Streaming API]
        Files[RTSA File Reader]
    end

    API --> Detection
    Detection -->|"File Path"| Files
    Detection -->|"HTTP URL"| HTTP
    Detection -->|"None / Default"| SDK
    SDK -.->|"Fallback"| HTTP
```

## 3. Source Selection

`AaroniaSource::new(config)` selects a backend using strict, deterministic rules:

| Configuration | Backend | Fallback | Platforms |
|---|---|---|---|
| `AaroniaConfig::from_file("recording.rtsa")` | File | None — fails if the file is missing | All |
| `AaroniaConfig::from_http("http://192.168.1.100")` | HTTP | None — fails if unreachable | All |
| `AaroniaConfig::default()` (nothing specified) | Native SDK | Polls the default HTTP endpoint (`localhost:54664`) if the SDK library is not found | All (SDK itself is Windows/Linux only) |
| Force option, e.g. `config.force_native_sdk()` | Forced type | None — fails if unavailable | Depends on forced type |

## 4. Backend Implementations

### 4.1 Native SDK

- Uses direct FFI calls into the Aaronia RTSA-Suite PRO shared libraries (`spectranv6` or `spectranv6eco`).
- **IQ mode constraint**: enforces `span * 1.5 <= receiverclock` by reading the live device clock before streaming.
- **Data path (RX)**: polls `GetPacket` on the C++ side every 5 ms and converts the raw pointer arrays to `Complex32` slices, zero-copy where structurally possible.
- **Data path (TX)**: provides an experimental `TxStream` API built on `SendPacket`. This path is hardware-unverified pending full V6 testing, as the ECO model lacks TX.
- **Sweep-spectra mode**: configures non-IQ sweeping via `SweepsaConfig` by modifying `"main/startfreq"`, `"main/stopfreq"`, `"main/rbw"`, and related keys. Supports the `"spectranv6/sweepsa"` and `"spectranv6eco/sweepsa"` open strings.
- **Health and GPS telemetry**: exposes `HealthState` and `GpsState` structs populated by a recursive configuration tree-walker (`walk_health_tree`) that fetches device diagnostics dynamically.

### 4.2 HTTP Streaming

- Implements the V9 RTSA HTTP specification.
- Supports Basic Auth and token-based authentication.
- **Formats**: requests and parses the `Int16` binary stream for maximum throughput, maintaining a persistent chunked-transfer buffer.
- Beyond streaming, `HttpEndpointsClient` exposes health telemetry, stream statistics, and device temperature, mirroring the desktop RTSA suite within a headless Rust process.

### 4.3 RTSA Files

- Maps the RTSA file format to the `SdrSource` interface.
- Extracts metadata (frequency, sample rate) directly from the file.
- Single-channel only; retuning is inherently unsupported for pre-recorded captures.

## 5. Channel Hopping and Dwell Control

The `sdr-source` trait implementation (`AaroniaSdrSource`) and its channel-hopping and dwell controllers are vendored and integrated natively into the crate. Automatic mid-stream retuning via `SourceConfig.channels_hz` is supported per backend:

- **Native SDK**: re-issues `configure_iq_receiver` with the new center frequency, applying the change to the open device handle via the `main/centerfreq` configuration key. No stream restart is required.
- **HTTP**: `AaroniaSource::set_center_frequency` wraps `HttpEndpointsClient::configure_capture(frequency_center=...)` as a `PUT` request. This requires the RTSA-Suite "Remote Config" license server-side; without it, the `PUT` returns success but is ignored.
- **File**: not supported.

Per-hop pacing uses the integrated `sdr_source::DwellController`, which inserts a ~75 ms settle after every retune to ride out the RTSA's apply-config latency and flush stale samples from the pipeline.

## 6. Overrun Detection and Fault Handling

The HTTP reader task (`connect_http` in `unified_source.rs`) runs a `DropDetector` over each packet's `start_time`/`end_time` metadata. A timestamp gap larger than the tolerance latches `AaroniaSource::pending_overrun`, which `take_overrun()` reads and clears. `single_channel_pump` and `hop_pump` (`sdr_source_impl.rs`) call `take_overrun()` once per emitted `IqPacket`, so a drop detected anywhere since the last read surfaces as `IqPacket::overrun = true` on the next packet. This is a per-call signal, not a precise per-sample one, since drop timing is lost once chunks merge into the flat `sample_buffer`.

The native-SDK and file backends do not populate this yet (`overrun` is always `false` for them). Native-SDK overrun detection would need to read the `WARN_OVERFLOW`/`WARN_DROPPED` packet flags in `native_sdk.rs`, which is Windows/Linux-only code.

The Aaronia capture thread (`AaroniaSdrSource::start`) is wrapped in `catch_unwind`, matching the USRP/HackRF/Pluto backends: a panic inside the pump loop is logged rather than silently unwinding the thread.

## 7. FutureSDR Integration

FutureSDR integration is available under the `futuresdr` feature gate. The crate exposes lower-level builder variants (e.g. `HttpSourceBuilder`) so `HttpSource` can be used as a source block in FutureSDR flowgraphs:

```text
┌─────────────────┐    ┌────────────────────┐
│   FutureSDR     │    │  sdr-aaronia-rs    │
│   Flowgraph     │    │    (Source API)    │
├─────────────────┤    ├────────────────────┤
│ HttpSource      │◄──►│ HttpSourceBuilder  │
│ (Block)         │    │ StreamingFormats   │
│                 │    │ StreamParser       │
└─────────────────┘    └────────────────────┘
```
