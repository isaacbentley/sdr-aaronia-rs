# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

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
