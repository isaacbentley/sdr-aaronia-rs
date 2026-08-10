# Contributing to sdr-aaronia-rs

First off, thank you for considering contributing to `sdr-aaronia-rs`! This document explains how the test suite is organized, which tools you'll need, and how to verify your changes locally before opening a pull request.

**Toolchain:** the crate uses the 2024 edition, so you need Rust 1.85 or newer. There is no pinned `rust-toolchain.toml`; CI tracks the latest `stable`.

## Quick Start

```bash
git clone https://github.com/isaacbentley/sdr-aaronia-rs.git
cd sdr-aaronia-rs

# Run the standard validation suite
cargo test                                        # unit + integration + properties (default features)
cargo test --test spec_coverage -- --nocapture    # spec inventory report
cargo clippy --all-features --all-targets -- -D warnings
cargo fmt --all --check
```

Note that a plain `cargo test` builds with the default features; suites gated on non-default features (e.g. `http_sink_test`, which needs `futuresdr`) are skipped. CI runs `cargo test --all-features`.

## Pre-push: mirror CI locally

Before pushing (or opening a PR), run the CI-parity script:

```bash
scripts/ci-local.sh
```

It reproduces the **exact** command each GitHub Actions job runs — fmt,
clippy, `cargo test --all-features`, `cargo deny --all-features check`,
`cargo hack check --each-feature --no-dev-deps`, `cargo machete`, and
the pinned-nightly Miri module suite — and fails fast with install
hints for any missing tool. Skip individual steps with e.g.
`SKIP="miri hack" scripts/ci-local.sh` when iterating.

The `--all-features` flags matter: several CI jobs validate the
*all-features* dependency graph and feature set, so a default-features
`cargo test` / `cargo deny check` passing locally does **not** imply CI
will pass. If you edit `.github/workflows/ci.yml`, update
`scripts/ci-local.sh` in the same commit (and vice versa), and run
`actionlint .github/workflows/ci.yml`.

Two steps exist because macOS builds skip the OS-gated native-SDK
modules entirely (`cfg(any(windows, linux))`), which CI's ubuntu and
windows legs do compile:

- **cross** — `cargo clippy --target x86_64-unknown-linux-gnu` /
  `x86_64-pc-windows-msvc` with `--features native-sdk`, via the rustup
  `stable` toolchain (install the targets with `rustup target add
  <triple> --toolchain stable`). Lib only — the gated *test* code needs
  a real Linux environment.
- **vm** — runs `cargo test --features native-sdk --lib` inside a Linux
  VM via Apple's `container` CLI (`container system start` first; the
  step auto-skips when the tooling isn't running). This is the only
  local step that compiles and runs `#[cfg(test)]` code inside the
  gated modules — a missed struct field in exactly such a test once
  shipped red to CI past every other local check.

Tiers 1–4 below are the core of the suite; the later tiers are optional but recommended if your pull request touches the parser, decompressor, or FFI surface.

## Test Pyramid

The crate is modeled on a layered test pyramid. Each tier catches a different class of bug; together they form the "definition of done" for our releases.

### 1. Unit Tests

Located alongside the code they test in `src/**/*.rs`. These cover format conversion, frequency parsing, builder defaults, struct sizes, and other basic utilities.

Run: `cargo test --lib`

### 2. Integration Tests (`tests/integration_test.rs`)

End-to-end tests against bundled `.rtsa` captures stored in Git LFS. The fixtures gracefully skip when LFS content isn't available. To run them fully, ensure you pull the LFS files:

```bash
git lfs pull
cargo test --test integration_test
```

Beyond the tiers named here, `tests/` also contains focused suites for the HTTP mock server (`http_mock_test.rs`), the HTTP sink (`http_sink_test.rs`, requires `futuresdr`), the C API (`c_api_test.rs`), the `sdr-source` implementation (`sdr_source_impl_test.rs`), RTSA negative cases (`rtsa_negative_test.rs`), CW fixtures (`test_cw_mag.rs`, `test_cw_meta.rs`), SDK library loading (`native_sdk_load.rs`), and opt-in live-hardware smoke tests (`live_smoke.rs`). All of these run as part of `cargo test` where their features and environment allow.

### 3. Property Tests (`tests/properties.rs`)

We use `proptest` to verify invariants of the parser, decompressor, and validator surfaces.

Run: `cargo test --test properties`

For a deeper verification pass (useful before submitting a PR):
```bash
PROPTEST_CASES=4096 cargo test --test properties --release
```

### 4. Invariant-Coverage Inventory (`tests/spec_coverage.rs`)

A single page enumerating which documented invariants the test suite enforces. **If you are adding a new invariant-bound test, please add a row to `ENFORCED` in `tests/spec_coverage.rs`.**

Run: `cargo test --test spec_coverage -- --nocapture`

### 5. Miri (Nightly)

Catches undefined behavior in safe Rust code. Because Miri refuses to interpret FFI, we only run it on the pure-Rust modules. CI pins `nightly-2026-01-01` because newer nightlies changed the `fadd_fast` intrinsic signature, which breaks the `futuredsp` dependency's build — use the same pin locally:

```bash
rustup toolchain install nightly-2026-01-01 --component miri
cargo +nightly-2026-01-01 miri test --lib decompression
cargo +nightly-2026-01-01 miri test --lib http_streaming
cargo +nightly-2026-01-01 miri test --lib utils
```

### 6. Code Coverage

Code coverage is reported to Codecov on PRs targeting `main` (uploaded from the Linux CI leg). Coverage is informational only and will not fail your build. To check coverage locally:

```bash
cargo install cargo-llvm-cov
cargo llvm-cov --html
open target/llvm-cov/html/index.html   # xdg-open on Linux, start on Windows
```

### 7. Criterion Benchmarks (`benches/`)

Three benchmark harnesses track the hot paths across releases. Benchmarks are run on demand to investigate performance implications of a PR.

```bash
cargo bench                                       # Run all benchmarks
cargo bench --bench parse_int16_packet            # Run a specific harness
```

### 8. Mutation Testing

`cargo mutants` runs in CI on a weekly cron (Mondays) and on manual dispatch, advisory-only. To run it locally:

```bash
cargo install cargo-mutants
cargo mutants --no-shuffle --in-place=false --timeout=180
```

### 9. ASAN / UBSAN

The C FFI boundary is exercised under AddressSanitizer and UndefinedBehaviorSanitizer via `tests/asan/c_smoke.c`. CI runs this on a weekly cron (Tuesdays).

```bash
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly
# On Debian/Ubuntu, the sanitizer runtimes are also needed:
sudo apt-get install llvm libubsan1
bash tests/asan/run_asan.sh
```
*(Note: ASAN scripts currently require a Linux environment.)*

### 10. Dependency Hygiene (CI-enforced)

Three additional CI jobs gate every PR; run their tools locally if your change touches dependencies or features:

```bash
cargo install cargo-deny cargo-hack cargo-machete
cargo deny check                                  # licenses/advisories/bans, driven by deny.toml
cargo hack check --each-feature --no-dev-deps     # every feature combination compiles
cargo machete                                     # no unused dependencies
```

## Adding a New Spec Invariant

When you encounter a documented invariant in `docs/*.md` that the test suite doesn't currently enforce:
1. Name the test `prop_<short_description>` or `spec_<area>_<behavior>`.
2. Add a `///` doc comment describing the invariant the test pins.
3. Add a row to the `ENFORCED` table in `tests/spec_coverage.rs`.
4. Run `cargo test --test spec_coverage -- --nocapture` and confirm your row appears.

## Code Style

We use standard `rustfmt` defaults. Please run `cargo fmt --all` before pushing.

Clippy is run with `-D warnings` in CI. If a lint is genuinely wrong for the situation, use a targeted `#[allow(...)]` with a brief comment explaining why. `unsafe` blocks carry the usual `// SAFETY:` invariant comments.

## Pull Requests

- **Commit messages:** Conventional-commits style is preferred but not required. Describe *why* the change is needed and *what* it changes.
- **Templates:** Please fill out the Pull Request template when opening a PR. It includes checkboxes for the CI validations and a mandatory AI-usage disclosure section.

## License

By contributing, you agree your contributions will be licensed under GPL-3.0-or-later, the same as the rest of the project.
