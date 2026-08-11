# Changelog

All notable changes to this project will be documented in this file.

## [v1.0.0] - 2026-08-10

### Breaking
- `RtsaMetadata` lost `device_name`, `stream_sample_rate`, and
  `stream_center_frequency`; `RtsaSource::stream_info()` now returns a
  3-tuple. These fields were only ever populated by a fabricated
  "proximity" STRM layout that no spec revision or capture uses; real
  files always produced `None`.
- `SdkConfig` and `AaroniaConfig` gained a public `receiver_channel`
  field (breaks struct-literal construction);
  `NativeSdkSource::configure_iq_receiver` takes the channel as a
  fourth parameter so retunes re-apply it.

### Added
- **Production Python Bindings (`python-aaronia`)**: Full PyO3 bindings exported with error translation, type-safe config, and zero-copy IQ streaming directly to PyArrow/NumPy arrays.
- **SoapySDR Plugin (`soapy-aaronia`)**: Added full C++ SoapySDR plugin built on the `sdr-aaronia-rs` C-API, including RX streaming, center frequency/sample rate controls, and `hasHardwareTime("GPS")` overrides.
- **Transmit (TX) Support**: Added `UnifiedSink` (implementing `AaroniaSink` for native SDK TX) to support bidirectional streaming.
- **Hardware Time Synchronization**: GPS time is now fetched securely from the telemetry `GpsState` tree and bubbled through the C-API to the SoapySDR Time API (`getHardwareTime("GPS")`).
- **Automated Release CI/CD**: Added PyPI trusted publishing for Python wheels (via `maturin-action`) and GitHub Release packaging for the SoapySDR plugin (across Linux `.so`, Windows `.dll`, and macOS `.dmg`).
- Receiver-channel selection (`RxChannel`: `Rx1`/`Rx2`/`Rx1And2`) on
  every native-SDK config surface, and true dual-channel capture via
  `read_samples_dual` on `NativeSdkSource`, `SdkSource`, and
  `AaroniaSource`, with a read-mode latch preventing silent mono/dual
  mixing.
- `StrmChunk::capture_start_offset`: RTSA files' undocumented STRM
  trailing double, verified as the stream-relative capture start;
  `start_time_ns` is now anchored with it so reported spans match the
  recorded data.
- `scripts/ci-local.sh`: local CI-parity gate incl. cross-target and
  Linux-VM coverage of the OS-gated native-SDK modules (the VM step
  runs CI's exact ubuntu-leg commands).
- `benches/deinterleave_dual_iq`: criterion bench pinning the
  dual-channel demux hot path (~1.5 Gsamples/s per core baseline).
- Parser-robustness property tests: `RtsaSource::open` on arbitrary
  byte soup and on structured DSFH/DSFT chunk soup must never panic.

### Fixed
- RTSA chunk parsing verified against the vendor spec and real
  captures: STRT alignment padding and size-versioned tail offsets,
  SPRV/ANTA fixed-field sizes, single standard STRM layout, official
  `DSST`/`DSSU`/`DSPT` enum numbering with `Unknown` degradation, and
  `mEndTime` treated as stream-relative duration.
- Compressed `DSPT_SPECTRA` chunks now error explicitly instead of
  returning compressed bytes reinterpreted as f32 spectra.
- Native SDK: corrupt-packet guards consume the packet before erroring
  (previously a corrupt head-of-queue packet livelocked every
  subsequent read); carry buffers are flushed on `stop_streaming`.

## [v0.3.5] - 2026-08-06

Documentation-and-polish release: every Markdown doc was audited against
the implementation and corrected. No library API changes (the only `src/`
edits are doc comments).

### Changed
- DESIGN.md restructured and de-duplicated; verified claim-by-claim
  against the code (external `sdr-source` crate not "vendored", settle
  timing attribution, real module/function names, HTTP format defaults,
  feature-gating and platform caveats).
- README corrected: install snippet version, builder capabilities,
  `futuresdr` feature-gating of `HttpSource`/`HttpSink`, Windows/Linux-only
  `native-sdk` note, `AARONIA_SDK_PATH` macOS scope, links to the `docs/`
  specifications and changelog.
- CONTRIBUTING now matches CI: pinned Miri nightly, the `cargo-deny` /
  `cargo-hack` / `cargo-machete` gates, mutants/ASAN cron schedules,
  toolchain floor (edition 2024), and the full test-suite inventory.
- docs/HTTPSPEC, docs/SDKSPEC, docs/FILESPEC corrected against the
  implementation (wire-format details, endpoint coverage including
  `GET /samples` and the `POST /sample` TX push, `sweepsa` open strings,
  real API paths, enum numeric values, parser limits) with explicit
  "known divergence" notes where only hardware can settle the truth.
- Clarified that HTTP retuning uses the license-free `/control` endpoint;
  the "Remote Config" license gates only `/remoteconfig` writes
  (stale doc comment on `set_center_frequency` fixed to match).
- Examples cleaned up: emoji removed, `noaa_scanner` doc header rewritten,
  `http_iq_quickstart` now takes the server URL as an argument instead of
  hardcoding a private hostname.
- Five assertion-free tests now actually assert.

### Fixed
- Stale comments: `native_sdk_load` symbol count, `spec_coverage` row
  pointing at a renamed property test, bench comments describing mmap and
  inverted `?scale=` semantics (the `parse_int16_packet` bench now passes
  an encode-side scale, matching live-verified server behavior).

## [v0.3.4] - 2026-07-31

### Fixed
- **Native SDK: the tail of every oversized IQ packet was discarded.**
  `NativeSdkSource::read_samples` copied `min(packet.num, max_samples)`
  samples and then consumed the *whole* packet via
  `AARTSAAPI_ConsumePackets`. The SDK picks its own packet size, unrelated
  to the caller's `max_samples`, so anything past the request was lost
  permanently — consuming returns the buffer to the SDK. `block_size` is
  caller-set with only a 1024 floor, so a modest block size against
  `AARTSAAPI_MEMORY_MEDIUM` packets left the IQ stream full of holes, which
  breaks the phase continuity downstream ZC correlation and CFO tracking
  depend on. Excess samples are now retained in the (previously dead)
  `sample_buffer` and returned by subsequent calls, mirroring the HTTP
  path's remainder handling. See `read_samples`' new "Carry-over" section;
  `get_sample_buffer_size()` now reports something real.
- **Native SDK: `read_samples(.., 0)` destroyed a packet.** A zero-sized
  request used to fetch a packet, copy nothing out of it, and consume it.
  It now returns early without polling.
- **`sdk_sink`: TX bursts were scheduled at the Unix epoch.** The FutureSDR
  sink block built every `TxBurst` with `start_time: 0.0`, contradicting
  `TxBurst`'s own documentation that the device schedules against its
  master stream clock. Timestamps are now derived from
  `AARTSAAPI_GetMasterStreamTime` with a 10 ms lead, falling back to the
  documented `PUSH` immediate-dispatch flag when the clock is unreadable.
  Hardware-unverified, like the rest of the TX path.
- **`http_source`: reported buffer capacity disagreed with the enforced
  one.** The cap was written twice — `saturating_mul(2).max(1)` where it
  was enforced, a bare `* 2` where `StreamStats` reported it. With
  `buffer_size: 0` (which the builder accepts) `buffer_level` could exceed
  `buffer_capacity`, and a consumer computing a fill ratio divided by zero.
- **Diagnostic previews truncated on byte offsets, not char boundaries.**
  `&text[..n]` panics when `n` lands inside a multi-byte UTF-8 character,
  and these previews print raw RTSA responses carrying device/mission/
  antenna names straight from user configuration — one non-ASCII character
  in a mission title was enough to panic a diagnostic run.

### Changed
- **`cargo clippy --all-targets` now compiles on a default checkout.**
  `tests/http_sink_test.rs` needs the `futuresdr` feature but declared no
  `required-features`, so the documented lint command failed with `cannot
  find module or crate futuresdr`.
- **`SdkConfig::timeout` and `SdkSinkConfig::timeout` documented as
  inert.** Neither is read by any code path; the only timeout in force on
  the read path is the fixed 500 ms `NativeSdkSource::READ_POLL_DEADLINE`.
  Both fields are kept — they are `pub` on a published crate — but now say
  so. Honouring them would change `read_samples`' signature, so that is
  left as a deliberate decision for a future major.

## [v0.3.3] - 2026-07-31

### Fixed
- **`broad_search_for_chunk` could loop forever.** The RTSA chunk scan did
  not terminate at EOF, so a file whose signature was absent and whose
  search bound exceeded its length spun indefinitely — reachable from
  `RtsaSource::open` on an untrusted file.
- **`verify_config_changes` leaked probed device state.** A failed
  read-back returned early without restoring the parameter it had just
  offset, leaving the device 1 dB from where it started. The restore now
  runs on every path out, and logs when both the read-back and the restore
  fail.

## [v0.3.2] - 2026-07-13

### Added
- **`DwellAdvice::channel_override` is now honored**, and the Remote Config
  license gate was dropped from hop mode — channel hopping works on
  unlicensed devices via the free `/control` endpoint.

## [v0.3.1] - 2026-07-11

### Fixed
- **Decompression: out-of-range `compression_factor` rejected up front.**
  `Decompressor::decompress` now rejects `compression_factor > 31` (the
  documented range per `docs/FILESPEC.md`, "1 to 31 for lossy factor")
  instead of letting it reach `dequantize`'s `1i32 << (compression_factor -
  1)`, which overflows the sign bit at 32 (silently producing a negative
  quantizer) and panics on overflow at 33+ in debug builds. This value can
  originate from a parsed HTTP-stream packet, so a malformed/corrupt header
  could previously reach the panic.
- **Decompression: over-produced coefficients truncated, not just
  under-produced ones.** `decompress` already zero-padded a coefficient
  stream shorter than `num_rows * num_cols`; it now also truncates a longer
  one, since `wave_transform_step` derives its own row count from the raw
  buffer length rather than the caller's declared dimensions and would
  otherwise silently operate on more rows than requested.
- **Aaronia capture thread now panic-guarded**, matching the driver crates
  (USRP/HackRF/Pluto): the `AaroniaSdrSource::start` capture thread body is
  wrapped in `catch_unwind`, so a panic inside it is logged instead of
  silently unwinding the thread with no diagnostic.
- **HTTP overrun detection wired to `IqPacket::overrun`.** The HTTP reader
  task now runs a `DropDetector` over each packet's timing metadata; a
  detected gap latches a flag that `AaroniaSource::take_overrun()` surfaces
  on the next `read_samples` call. `single_channel_pump`/`hop_pump` in
  `sdr_source_impl.rs` now report real overrun status instead of
  hardcoding `false`. The native-SDK and file backends still report `false`
  (unchanged).
- **`aaronia_source_read_samples` (C API) copy bound hardened.** The FFI
  sample-copy now clamps to `temp_samples.len()` in addition to the
  caller-supplied capacity, so a hypothetical future miscount from
  `read_samples` can't cause an out-of-bounds read; added a compile-time
  size assertion between `FfiComplex` and `Complex32` guarding the
  reinterpret cast.
- **`orecchiette-sdr-source-rs` dependency floor** bumped from `0.1.0` to
  `0.1.2` to match every sibling crate's declared floor.

## [v0.3.0] - 2026-07-11

### Removed
- **BREAKING: `file_performance` module removed** (`MmapRtsaReader`,
  `AdaptiveChunkReader`, `AccessStats`, `CacheStats`, `ChunkType`, and the
  `memmap2` dependency). This tiered-cache, adaptive-read-ahead
  memory-mapped reader was never wired into the crate's actual file-reading
  path — `RtsaSource` (the real hot path, used by every file-source
  consumer) has always read via buffered `std::io` and never touched this
  module. It had zero callers anywhere in the crate outside its own tests.
  If you were depending on these types directly, buffered access through
  `RtsaSource` covers the same file-reading needs; there is no drop-in
  replacement for the standalone mmap/cache API itself.

### Fixed
- **`HttpSourceBuilder` doc comment misplacement:** the "no-op kept for
  backward compatibility" doc comment was attached to `with_shared_stats`
  (which is not a no-op — it wires up the retune/stats-sharing mechanism)
  instead of `with_native_sdk` (the actual no-op immediately below it).
  Moved to the correct method and gave `with_shared_stats` an accurate doc.
- **`HttpSink` dropped-sample accounting:** `push_batch` now counts a batch
  as dropped when the background sender task's channel is closed (e.g. the
  task panicked), not just on a failed/timed-out HTTP push. Previously this
  failure mode silently discarded samples without incrementing
  `dropped_samples()` — the documented way to detect a persistently broken
  TX link. `HttpSink` also now aborts its background sender task on drop
  instead of relying on the channel closing to signal it.

### Changed
- **Native SDK error handling finished:** the last four `NativeSdkClient`
  methods that still hand-rolled their own `AARTSAAPI_Result` check
  (`enum_device`, `config_first`, `config_next`, `get_packet`) now route
  through the `check_res`/`Error::SdkApi` path introduced in v0.2.6,
  matching every other method in the client. Behavior is unchanged for
  callers (`AARTSAAPI_EMPTY` still maps to `Ok(None)`/`Ok(false)`); failures
  now carry a structured `SdkError` instead of a formatted string.
- **De-duplicated `device_family`/`device_open_mode`:** `SdkConfig` and
  `SdkSinkConfig` shared byte-for-byte identical device-type-splitting
  logic (differing only in the default open-mode suffix, `raw` vs.
  `iqtransmitter`). Extracted into `native_sdk::split_device_type`; both
  public methods keep their existing signatures.

## [v0.2.6] - 2026-07-10

### Added
- **Native SDK Transmitter (TX):** Added transmit capabilities through the C++ Native SDK (`SdkSink` and `SdkSinkConfig`). This provides TX path parity with the `SdkSource` and includes FutureSDR integration (`SdkSinkBlock`).
- **Examples:** Added `examples/native_sdk_transmit.rs` to demonstrate programmatic device configuration, master stream time-driven packet pacing, and sending a continuous LoRa-like CSS up-chirp.

### Changed
- **Granular Errors:** Migrated the opaque `Error::Sdk(String)` to a structured `Error::SdkApi { operation: &'static str, code: SdkError }` to allow programmatic error recovery.
- **Warning Isolation:** The Native SDK C++ FFI bindings now correctly categorize and log warnings (codes with the `0x40000000` bit set) rather than escalating them to fatal errors, mapping closely to the official Aaronia Java/C++ driver behaviors.

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
