# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

## [v0.2.4] - 2026-07-10

### Performance
- **HTTP stream parser:** binary formats (int16/float16/float32) now
  deserialize `PacketMetadata` straight from the header bytes instead of
  building — and discarding — a full `serde_json::Value` DOM per packet.
  `parse_int16_packet` improves ~41% at 256 samples/packet down to ~6% at
  64k (the header cost amortizes over larger payloads). The pure-JSON
  stream still uses the DOM, since it reads sample values from it.
- **Float32 IQ parse:** on little-endian hosts the payload is bulk-copied
  in one `memcpy` into an aligned `Vec<Complex32>` instead of decoding each
  `f32` individually (portable per-element fallback on big-endian).
- **RTSA file replay:** IQ/spectra sample reads now do one bulk `read_exact`
  + in-memory decode instead of two `read_f32`/`read_i16` calls per sample.
- **Native SDK `read_samples`** logs per-read at `trace!` instead of
  `info!`, so an enabled info subscriber no longer pays formatting cost on
  the hot path.
- Minor: `HttpSource` bulk-`extend`s its sample ring buffer rather than
  pushing element-by-element; the unified HTTP source reuses one
  connection-pooled client for its reader task instead of building a
  second.

### Fixed
- **Native SDK `read_samples` soundness:** the wide-stride (`stride > 2`) IQ
  gather path underflowed `usize` when `max_samples == 0`, handing
  `slice::from_raw_parts` a ~`usize::MAX` length (UB). It now skips the empty
  case, `packet.num` is bound-checked before the `* 2` to avoid an `i64`
  overflow on a malformed packet count, and `packet.stride` is sanity-bounded
  (a corrupt oversized stride would otherwise size an out-of-bounds slice).
- **`AaroniaSource` HTTP streaming-task leak:** the background `/stream` reader
  is now aborted on `stop_streaming()` and on drop, instead of lingering
  (holding the open connection, so the device kept streaming) until the next
  packet arrived.
- **`Decompressor::decompress`** rejects zero `num_rows`/`num_cols` instead of
  spinning the inverse-wavelet loop forever / dividing by zero.
- **`MmapRtsaReader::read_chunk`** bounds check uses `checked_add`, so a
  pathological offset can't wrap past it into an out-of-bounds slice.
- **`HttpSourceBuilder`** default reference level corrected from `+20 dBm` to
  `-20 dBm` (matching `AaroniaConfig`); the old default desensitized the
  receiver. The mislabeled "dB" unit comment is fixed too.
- **`HttpSource::work`** now requests re-scheduling (`io.call_again`) after a
  stream-error reconnect.

### Changed
- **`HttpSink` `timeout_ms`** is now honored: each transmit push is bounded by
  it (timed-out batches count as dropped) and `HttpSinkBuilder::timeout_ms`
  exposes the knob. Previously the value was silently ignored.
- README install snippet updated to the current `0.2` version line.

## [v0.2.3] - 2026-07-05

### Fixed
- **HTTP source now tunes the device on startup.** `init_http_source` was opening `/stream` without first sending a `/control` capture-configuration request, so the SDR always streamed whatever frequency the RTSA Suite happened to be configured to (typically 300 MHz) — completely ignoring the caller's `center_frequency` and `span_frequency`. A `configure_capture` call is now issued before the stream is opened, matching the Native SDK path's behaviour.

### Changed
- `http_iq_quickstart` example: added CLI arguments for frequency and sample rate, periodic signal-power logging to stderr, and fixed a clippy `needless_borrow` lint.

## [v0.2.2] - 2026-07-05

### Changed
- Dropped explicit MSRV (Minimum Supported Rust Version) policy to track the latest `stable` Rust compiler.
- Updated `.github/workflows/ci.yml` to compile tests, clippy, and coverage against `stable` instead of hardcoding an older MSRV. This prevents transitive dependencies that bump their MSRVs from breaking CI.

## [v0.2.1] - 2026-07-05

### Added
- Native SDK support for configurable receiver channel (`Rx1`, `Rx2`, `Rx1And2`).
- Support for setting HTTP wire format and stream scale in `AaroniaConfig` and `AaroniaSourceBuilder`.
- Updated `README.md` with a quickstart guide and more detailed examples.

### Fixed
- Fixed CI build failures by bumping MSRV to 1.86 and allowing the `Zlib` license in cargo-deny.
- Changed default HTTP streaming format to `Float32` instead of `Json` to fix high-bandwidth crashes.
- Resolved `FutureSDR` block execution deadlocks inside `HttpSink` by offloading `reqwest` synchronous HTTP I/O into a detached `tokio` background task.

## [v0.1.1] - 2026-07-03

### Changed
- Disable `futuresdr` on docs.rs and bump to 0.1.1
- Fix license badge by using GitHub endpoint instead of crates.io
- Update Cargo.lock for 0.1.1

## [v0.1.0] - 2026-07-03

### Added
- Initial release.
