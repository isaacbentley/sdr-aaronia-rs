# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- **Connect retry with backoff.** Reaching the RTSA server (the `/info`
  probe and the initial tuning PUT) now retries transient failures up to
  4 times with exponential backoff (250 ms → 2 s), under a 10 s total
  budget so that slow-failing attempts can't stall startup. A cold
  `*.local` hostname refuses the first connection while mDNS resolves,
  which previously surfaced as "Is it running and accessible?" against a
  server that was up. 4xx and config errors still fail on the first
  attempt.
- **Configurable blocking-read timeout** — `AaroniaConfig::read_timeout`
  (default 30 s, previously a hard-coded literal), with builder setters,
  a Python `read_timeout` property (seconds), the C API
  `aaronia_source_builder_read_timeout_us`, and a `read_timeout=<seconds>`
  SoapySDR device arg. `read_samples_deadline` (and therefore the
  SoapySDR and seify paths) continues to use its caller's per-call
  deadline.

- **Automatic HTTP stream reconnection, enabled by default**
  (`AaroniaConfig::auto_reconnect`). An RTSA-Suite restart or a brief
  network drop used to end a session permanently — the reader task
  exited and every later read returned `Error::Protocol`. The reader now
  reopens the stream up to 5 times with exponential backoff (~8 s total),
  re-applies the current tuning (a restarted server comes back on its
  mission's frequency, which would otherwise stream the wrong band
  unnoticed), resyncs the drop detector, and flags the first packet after
  the gap as an overrun. The attempt budget resets only after a
  connection has stayed up for 30 s, so a server that accepts and
  immediately hangs up can't reconnect forever. Exhausting the attempts,
  or setting `auto_reconnect(false)`, reproduces the old fail-fast
  behaviour exactly. Exposed as a Python `auto_reconnect` property, the
  C API `aaronia_source_builder_auto_reconnect`, and a `reconnect=0|1`
  SoapySDR device arg.
- `DropDetector::resync()` — forgets the last packet's timestamp while
  keeping the cumulative counters, for use across a deliberate
  discontinuity. `reset()` zeroes the counters too, which would make the
  monotonic total consumers read jump backwards after a reconnect.

### Fixed
- **Channel hopping could stall for up to the read timeout.** `hop_pump`
  read with `read_samples`, which waits for a full block or 30 s — vastly
  longer than a 20-40 ms dwell — so a stalled server (or, once
  auto-reconnect landed, a stream working through its backoff) held the
  pump and starved every remaining hop. It now reads with
  `read_samples_deadline` bounded by the dwell deadline it already
  computes.

### Documentation
- **Python type stubs.** `python-aaronia/aaronia.pyi` documents the full
  binding surface, and maturin packages it with the required `py.typed`
  marker, so editors and type checkers now understand the module.
  Verified by building the wheel and inspecting its contents, by
  `mypy --strict`, and by comparing every declared name against the
  installed module.
- **Hardware-verification matrix in the README.** Every capability is now
  labelled live-verified, mock-tested, or hardware-unverified, with the
  development device stated outright (SPECTRAN V6 ECO, single RX, no TX
  licence, driven over HTTP from macOS). The TX, dual-channel and
  native-SDK paths have never been exercised against hardware, stated in
  one place rather than scattered across five documents.
- **`docs/QUICKSTART.md`** — the RTSA-Suite mission configuration that
  everything depends on and that no document previously covered: adding
  the HTTP Server block, connecting the device output to it, verifying
  with `curl`, first samples in Rust/Python/SoapySDR, and the common
  failure modes (span versus sample rate, partial retune PUTs, network
  saturation at Float32 rates).
- **`docs/APPS.md`** — per-application setup for SDR++, GQRX, GNU Radio
  and SoapySDR-from-Python, including that the single `REF` gain element
  is a reference level in dBm, where raising it *reduces* sensitivity.
- **Prebuilt install instructions** for the SoapySDR plugin: every
  release already attached built modules, but the README only explained
  building from source with CMake and a Rust toolchain.
- **`docs/USAGE.md`** — the worked examples that previously made up 60%
  of the README, moved out of it and now compiled as doctests so they
  cannot drift from the API. The README keeps the quickstart and points
  to the guides (516 lines down to 291).
- Rust snippets in `docs/QUICKSTART.md` are compile-checked as doctests
  via a `cfg(doctest)` module, so the guides can't drift from the API.

### CI
- **Release assets are now self-describing archives.** The plugin
  shipped as bare `libaaroniaSupport.so`, `aaroniaSupport.dll` and a
  `.dmg`, none of which stated a version, operating system,
  architecture, or that they were SoapySDR plugins. Releases now attach
  `SoapyAaronia-<version>-<os>-<arch>.tar.gz` (`.zip` on Windows), each
  containing the module, install instructions and the licence. The
  `.dmg` is gone; it was an unusual container for a single plugin file
  and did not avoid macOS quarantine. Per-platform archives are kept
  deliberately, since a combined one would make every user download all
  three modules to obtain one. Anything downloading the previous bare
  filenames (`libaaroniaSupport.so`, `aaroniaSupport.dll`,
  `SoapyAaronia.dmg`) by name needs updating to fetch and unpack the
  archive instead.
- The Linux module is built with debug symbols stripped, which is where
  its roughly 19 MB against the Windows build's 5 MB came from, and the
  release job verifies the stripped module still loads via
  `SoapySDRUtil --check` before publishing it.
- GitHub releases now lead with this file's entry for the tag being
  released, instead of only an auto-generated commit list (which is
  still appended). A missing entry degrades to a note rather than
  failing the release.

## [v0.5.1] - 2026-08-11

### Fixed
- **Mid-stream HTTP retuning was silently broken on real hardware.**
  Live testing against RTSA-Suite PRO (SPECTRAN V6 ECO) showed the
  `/control` capture endpoint applies a frequency change only when
  `frequencyCenter` and `frequencySpan` are both present; a lone
  frequency field returns `{"success":true}` but is ignored, so
  `set_center_frequency` / `set_span_frequency` (and therefore hop-mode
  retuning and the SoapySDR/seify/Python retune surfaces over HTTP)
  reported success while the device kept streaming at the old tuning.
  All HTTP retune paths now send the complete capture tuple (center,
  span, reference level), `configure_capture` warns on lone-frequency
  payloads, and a live smoke test (`live_retune_full_tuple_applies`)
  plus a span-required mock predicate guard the regression.
  `referenceLevel`-only PUTs were unaffected (they apply on their own).
- Corrected the docs that mis-attributed the silent-ignore behavior to
  the Aaronia "Remote Config" license (READMEs, DESIGN.md, HTTPSPEC.md,
  `probe_remote_config_license` doc comments): retuning goes through the
  license-free `/control` endpoint; the license gates only
  `/remoteconfig` writes.

### CI
- Clippy now runs with `--workspace` (CI, `scripts/ci-local.sh`, and the
  Linux-VM leg): `python-aaronia` is a workspace member that no job
  linted, and it had accumulated 12 unflagged clippy errors — a genuine
  `type_complexity` (fixed with a type alias) and 11 `useless_conversion`
  reports from pyo3 0.22's `#[pymethods]` wrapper codegen (crate-level
  allow, documented for removal at pyo3 ≥ 0.23).

## [v0.5.0] - 2026-08-11

Breaking 0.x release combining the RTSA file-format verification work
with the new binding/plugin surfaces. (The recorded floor for this
range was 0.4.0; it ships as 0.5.0 to reflect the added Python/SoapySDR/
TX/GPS feature scope. A branch briefly relabelled it 1.0.0 — a 1.0
stability commitment on top of never-hardware-verified TX and freshly
reviewed bindings is exactly what 1.0 must not be.) The new surfaces
(Python bindings, SoapySDR plugin, TX, GPS time) shipped through a
six-stream production review with live-hardware validation of the RX
paths; hardware-facing TX/dual-channel paths remain
**hardware-unverified** where noted in the docs.

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
- **Python bindings (`python-aaronia`)**: PyO3 bindings (module
  `aaronia`, abi3 wheels for CPython 3.9+) with typed exceptions mapped
  from the crate's error enum, readable/settable config covering HTTP
  URL, file playback, serial, format, scale, reference level and
  receiver channel, GIL-released blocking calls, live retuning, and
  single-copy reads into NumPy or PyArrow arrays (including
  dual-channel reads).
- **SoapySDR plugin (`soapy-aaronia`)**: C++ plugin over the C API —
  RX streaming in CF32/CS16 with honoured `timeoutUs` and partial
  reads, retune-safe locking, per-direction stream handles, discrete
  sample-rate listing, device args (`url`, `file`, `serial`, `freq`,
  `rate`, `ref_level`, `format`, `scale`, `rx_channel`), truthful
  GPS hardware-time probing, and TX (hardware-unverified) via the
  native SDK on Windows/Linux.
- **Transmit support**: `UnifiedSink` + `aaronia_sink_*` C API driving
  the native-SDK TX path (device open/configure/start, master-clock
  burst timing, caller-controlled packet-boundary flags).
  **Hardware-unverified**; unavailable off Windows/Linux and errors
  clearly there.
- **GPS time**: `get_gps_time` from the SDK telemetry tree, over FFI,
  with validity gating; native-SDK backend only.
- **Release automation**: maturin wheels + PyPI trusted publishing and
  SoapySDR plugin packaging in release.yml (first exercised on this
  branch's CI, not at tag time).
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
