# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Changed
- **The Homebrew formula is no longer a release asset.** It is uploaded
  as the `homebrew-formula` workflow artifact instead. Homebrew 4 and
  later cannot install from a formula URL, so on the release page it
  was a file no user could act on, listed among ones they can. Its only
  reader is whoever updates the tap, and the checksums are still
  rendered against the published archives so none are computed by hand.

### Documentation
- **Measured what the 80% usable-bandwidth figure actually is, and
  explained it in one sentence.** Sample rate and RF bandwidth were
  described as related by a ratio without saying what the ratio was or
  where it came from. RTSA declares exactly 0.8 x Fs as the packet's
  frequency range at every rate — a fixed rule, checked at 61.44,
  15.36, 7.68 and 3.84 MHz — and every sample still arrives, so an FFT
  spans the whole rate while only that 80% is flat and calibrated.
  Sweeping the receiver's own noise floor on a V6 ECO confirms it: the
  response is flat within 0.5 dB across 0.80 of the rate at 15.36 MHz
  sampling and 0.89 at 7.68 MHz, and at full span the analog filter is
  about 1 dB down by the declared edge — which is where Aaronia's
  44 MHz data-sheet figure comes from, against the 49.152 MHz the
  device itself declares. The guidance readers need is one line: to see
  N Hz of spectrum, sample at N / 0.8.
- **The READMEs stated the V6 ECO's sample-rate ladder as if it were
  every device's.** "The device runs a fixed ladder of rates: 61.44 MHz
  halved down to 120 kHz" is measured and true for an ECO, but a full
  V6 selects its receiver clock and starts higher — by how much is the
  open question the specs already record, and the user-facing pages did
  not carry it. The Python, SoapySDR, quickstart and applications docs
  now say whose ladder it is and point at the note, and
  `iq_sample_rates` says so in its own documentation.
- The SoapySDR README gained a Sample rates section: why
  `listSampleRates` reports the real ladder, that `setSampleRate` snaps
  and logs, and that `getSampleRate` while streaming is the number to
  trust on hardware the advertised ladder does not describe.
- SDKSPEC still gave the eco family's clock as 61.44 MHz in a second
  place, contradicting the correction made elsewhere in the same
  document. It is 92.16 MHz; 61.44 MHz is the top IQ rate.
- The same sweep caught the claim in `unified_source`'s own
  documentation, in the `/control` span note in HTTPSPEC, and in the
  0.8 usable-bandwidth ratio, all of which read as universal and are
  measurements from one device. `seify_impl`'s sample-rate range is
  capped at 61.44 MHz for the same reason and now says so: seify has no
  device handle at that point to ask for something better.

## [v0.7.4] - 2026-08-12

### Fixed
- **The SoapySDR plugin ignored an unrecognised `format=` silently.**
  A device string carrying `format=int16` — the wire name rather than
  the plugin's `I16` — streamed the default format while claiming
  otherwise. It now warns and continues. The server behaves the same
  way and worse: an unrecognised `format=` on `/stream` serves the
  RTSA file format with HTTP 200 rather than an error, so a typo
  changes the wire format entirely. `raw16`, which Aaronia's own Qt
  reference client sends, is a working alias for `int16`.

### Documentation
- Checked Aaronia's V6 remote control notes (rev 4, May 2026) against
  the hardware. `/remoteconfig` enum fields take an index as well as a
  label; one `simpleconfig` PUT can carry several groups, and groups
  other than `main` work; and a PUT naming a block that is not in the
  mission returns 200 and changes nothing, which
  `simple_remote_config` now warns about since it reports `Ok(())` for
  a write that did not happen. In the config-tree form the receiver
  name is ignored altogether — the write is routed by `config.name`.
- Documented loading a mission over `/control`, and that every
  `/control` payload needs its `type` or the server answers `400`.
  Loading a mission is deliberately not exposed by the crate: swapping
  the mission under a running capture should be a caller's decision,
  not a side effect.
- Noted that RTSA-Suite has no status endpoint. Aaronia's own liveness
  check reads the `404` from `/api/status` as proof the server is up.
- **What "Full" means on a full V6 is unresolved.** A V6 ECO follows the
  SDK's `spanfreq <= receiverclock / 1.5`, measured. Aaronia's Remote
  Config screenshots show a full V6 at a 92 MHz clock delivering
  92.16 MHz of IQ samples per second at span "Full" — the clock itself.
  `iq_sample_rates_for_clock` may therefore understate the top of the
  ladder by 1.5x for a full V6 at a non-default clock; it says so now.
  Settling it needs a full V6.
- HTTPSPEC contradicted itself on the Remote Config licence, asserting
  in one section that writes need it and in another that a live
  unlicensed system accepts them. The second is what the hardware does,
  re-confirmed for centre frequency, decimation, reference level and
  the preamplifier.
- SDKSPEC still gave the V6 ECO's receiver clock as 61.44 MHz, which
  0.6.2 corrected in code to 92.16 MHz. 61.44 MHz is the ECO's top IQ
  rate, that clock over 1.5; the document had the two confused.
- **A marker stream is not a categories packet**, which the previous
  draft of this entry got wrong. Aaronia's example declares
  `payload: "spectra"`, and spectra samples are a 2D array, so its
  nesting is correct for what it says it is. Its three frequency fields
  are all zero, so the category names and ranges are the only
  description of what the numbers mean.
- Aaronia's endpoint specification (rev 11) settles several things this
  document had only inferred, and their support answers go further.
  `/control` takes PUT only, and a command reaches every block that
  understands it unless `receiverUUID` or `receiverName` scopes it —
  the specification says such commands cannot be addressed to a block,
  which their support corrected in 2024. The per-type settings are now
  listed in full, including `deviceconnect` and `camera`, which this
  crate does not model. Zones cannot be configured remotely at all.
  The server starts dropping data once its outbound TCP buffer passes
  8 MB, which is the mechanism behind most unexplained gaps.
  `/healthstatus` is organised as `info`, `status`, `health`,
  `settings` and `components`, the last being how satellites attached
  over HTTP appear in a local tree.
- **`status/iqsamples` is the native rate, not the delivered one.** It
  held at 61.44 MHz while the same device delivered 15.36, then 7.68,
  then 61.44 MS/s. It looks like a sample rate and is not the one your
  stream is running at; read `sampleFrequency` from packet metadata.
  Documented the other fields a V6 ECO reports alongside it.
- **One HTTP Server and one HTTP Client connection are free**;
  additional instances and connections are licensed separately, as are
  Stream Merger and Stream Splitter. Running this crate and a second
  client against one server at the same time is a second connection —
  the licence limit most likely to be met in practice, and unrelated to
  Remote Config.

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
