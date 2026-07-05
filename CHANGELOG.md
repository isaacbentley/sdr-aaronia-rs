# Changelog

All notable changes to this project will be documented in this file.

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
