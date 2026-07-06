# Changelog

All notable changes to this project will be documented in this file.

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
