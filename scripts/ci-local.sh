#!/usr/bin/env bash
# Local mirror of .github/workflows/ci.yml — run this before pushing.
#
# Every step below reproduces the *exact* command its CI job runs, so a
# clean run here means the corresponding GitHub Actions job passes too
# (modulo OS differences; see the notes on each step). When editing
# either file, keep the two in sync — a step that drifts from CI is
# worse than no step, because it manufactures false confidence.
#
# The canonical example of why parity matters: `cargo deny check` and
# `cargo deny --all-features check` validate *different dependency
# graphs*. CI's deny action defaults to `--all-features`; a plain local
# check once reported the Zlib/MPL-2.0 allowances as stale, their
# removal passed locally, and CI promptly failed on the feature-gated
# crates that need them.
#
# Also mirrors ci.yml when reused as the release validation gate.
# Usage:
#   scripts/ci-local.sh            # run everything (recommended pre-push)
#   SKIP="miri hack" scripts/ci-local.sh   # skip named steps
#
# Stable/nightly split, as in ci.yml: FutureSDR 0.10 builds only on
# nightly (E0554 on stable), so the stable steps name every feature but
# `futuresdr` ($STABLE_FEATURES) instead of passing --all-features, and
# the `nightly` step runs the --all-features checks on the pinned
# $NIGHTLY_TOOLCHAIN.
#
# Tools beyond stable cargo: cargo-deny, cargo-hack, cargo-machete, the
# pinned futuresdr nightly (with clippy) and the pinned miri nightly.
# The preflight check below prints the install command for anything
# missing.

set -euo pipefail
cd "$(dirname "$0")/.."

# Property tests: same case count CI uses.
export PROPTEST_CASES="${PROPTEST_CASES:-256}"

# Pinned to match the miri job in ci.yml (see the comment there about
# the futuredsp `fadd_fast` breakage on current nightlies). Bump both
# together.
MIRI_TOOLCHAIN=nightly-2026-01-01

# ci.yml's STABLE_FEATURES (every feature except `futuresdr` and
# `futuresdr-audio`; release.yml's semver-checks step carries a copy too)
# and the `nightly` job's pinned toolchain. Bump/extend them together.
STABLE_FEATURES=http,file,ffi,tls,seify,native-sdk
NIGHTLY_TOOLCHAIN=nightly-2026-11-21

SKIP="${SKIP:-}"
skipped() { [[ " $SKIP " == *" $1 "* ]]; }

step() { printf '\n\033[1m=== %s ===\033[0m\n' "$*"; }

# ---- preflight: fail fast with install hints, not mid-run ----------
missing=0
require() { # require <step> <probe...> -- <install hint>
    local name="$1"; shift
    local hint="${*: -1}"
    if skipped "$name"; then return 0; fi
    if ! "${@:1:$#-2}" >/dev/null 2>&1; then
        echo "missing tool for step '$name' — install with: $hint" >&2
        missing=1
    fi
}
# Miri must run with the pinned toolchain's own cargo. Neither
# `rustup run` nor `cargo +toolchain` is reliable when a non-rustup
# cargo (e.g. Homebrew's) shadows the rustup shim on PATH — both end in
# "no such command: miri" — so resolve the toolchain bin dir explicitly
# and prepend it.
miri_bin() { dirname "$(rustup which --toolchain "$MIRI_TOOLCHAIN" cargo 2>/dev/null)"; }
# Same for the futuresdr nightly.
nightly_bin() { dirname "$(rustup which --toolchain "$NIGHTLY_TOOLCHAIN" cargo 2>/dev/null)"; }

require deny    cargo deny --version           -- "cargo install cargo-deny"
require hack    cargo hack --version           -- "cargo install cargo-hack"
require machete cargo machete --version        -- "cargo install cargo-machete"
require miri    env PATH="$(miri_bin):$PATH" cargo miri --version \
        -- "rustup toolchain install $MIRI_TOOLCHAIN --component miri"
require nightly env PATH="$(nightly_bin):$PATH" cargo clippy --version \
        -- "rustup toolchain install $NIGHTLY_TOOLCHAIN --component clippy"
require nightly cargo hack --version           -- "cargo install cargo-hack"
if [[ $missing -ne 0 ]]; then
    echo "aborting before running anything; install the tools above or SKIP the steps." >&2
    exit 1
fi

# ---- test job (fmt + clippy + tests; CI splits these across OSes) --
if ! skipped fmt; then
    step "cargo fmt --all --check"
    cargo fmt --all --check
fi

if ! skipped clippy; then
    # --workspace pulls in python-aaronia, which no other step lints.
    # Not --all-features: `futuresdr` needs nightly (the nightly step).
    step "cargo clippy --workspace --features $STABLE_FEATURES --all-targets -- -D warnings"
    cargo clippy --workspace --features "$STABLE_FEATURES" --all-targets -- -D warnings
fi

# ---- untagged markdown fences --------------------------------------
# `src/lib.rs` pulls these three files in with `include_str!`, and
# rustdoc compiles an untagged ``` fence as Rust. That is how an ASCII
# architecture diagram once became a doctest failure on `main`: the
# Linux CI leg runs `cargo llvm-cov`, which skips doctests, so only the
# macOS and Windows legs caught it. Cheaper to catch here.
if ! skipped fences; then
    step "untagged code fences in the included markdown"
    awk '
        /^```/ {
            n++
            if (n % 2 == 1 && $0 == "```") {
                printf "%s:%d: untagged fence — rustdoc will compile this as Rust\n", FILENAME, FNR
                rc = 1
            }
        }
        END { exit rc }
    ' README.md docs/QUICKSTART.md docs/USAGE.md PLUGINS.md docs/SYNC.md
fi

if ! skipped test; then
    # CI's Linux leg wraps this in cargo-llvm-cov for coverage; the
    # test outcome is identical, so plain `cargo test` suffices here.
    step "cargo test --features $STABLE_FEATURES  (PROPTEST_CASES=$PROPTEST_CASES)"
    cargo test --features "$STABLE_FEATURES"
fi

# ---- nightly job (--all-features, futuresdr included) --------------
# CI runs this on ubuntu + windows; on macOS --all-features silently
# skips the native-sdk modules, SdkSinkBlock included (the vm step
# covers the Linux side). The hack pair is CI's ubuntu-only remainder
# of the stable hack job: `futuresdr` and `futuresdr-audio`, each on its
# own.
if ! skipped nightly; then
    step "cargo clippy/test --all-features + hack (futuresdr, futuresdr-audio alone) on $NIGHTLY_TOOLCHAIN"
    PATH="$(nightly_bin):$PATH" cargo clippy --workspace --all-features --all-targets -- -D warnings
    PATH="$(nightly_bin):$PATH" cargo test --all-features
    PATH="$(nightly_bin):$PATH" cargo hack check --each-feature --no-dev-deps --include-features futuresdr,futuresdr-audio
    PATH="$(nightly_bin):$PATH" cargo hack check --each-feature --all-targets --include-features futuresdr,futuresdr-audio
fi

# ---- deny job ------------------------------------------------------
if ! skipped deny; then
    # The EmbarkStudios/cargo-deny-action@v2 default arguments include
    # --all-features; a default-features check is NOT equivalent.
    step "cargo deny --all-features check"
    cargo deny --all-features check
fi

# ---- hack job ------------------------------------------------------
if ! skipped hack; then
    # futuresdr and futuresdr-audio excluded: the nightly step checks them
    # on their own.
    step "cargo hack check --each-feature --exclude-features futuresdr,futuresdr-audio (--no-dev-deps + --all-targets)"
    cargo hack check --each-feature --no-dev-deps --exclude-features futuresdr,futuresdr-audio
    cargo hack check --each-feature --all-targets --exclude-features futuresdr,futuresdr-audio
fi

# ---- machete job ---------------------------------------------------
if ! skipped machete; then
    step "cargo machete"
    cargo machete
fi

# ---- miri job (FFI-free module allowlist, pinned nightly) ----------
if ! skipped miri; then
    step "cargo miri test (decompression, http_streaming, utils) on $MIRI_TOOLCHAIN"
    # --ignore-rust-version: the pinned nightly predates the crate's
    # `rust-version`, as in ci.yml.
    PATH="$(miri_bin):$PATH" cargo miri test --ignore-rust-version --lib decompression
    PATH="$(miri_bin):$PATH" cargo miri test --ignore-rust-version --lib http_streaming
    PATH="$(miri_bin):$PATH" cargo miri test --ignore-rust-version --lib utils
fi

# ---- OS-gated native-sdk code (compiled only on CI's linux/windows) -
# On macOS, `--all-features` silently skips src/native_sdk.rs and
# friends (cfg windows/linux), so the steps above never see them; CI's
# ubuntu/windows legs do. Cross-clippy the lib for both targets. This
# cannot cover *test* code in the gated modules (`--all-targets` pulls
# dev-deps whose C build scripts need a cross C toolchain); the vm step
# below closes that remainder.
if ! skipped cross; then
    step "cross-clippy native-sdk lib (x86_64 linux + windows, rustup stable)"
    # Same PATH pinning rationale as miri_bin: a non-rustup cargo on
    # PATH has no cross std for these targets.
    rustup_bin="$(dirname "$(rustup which --toolchain stable cargo)")"
    PATH="$rustup_bin:$PATH" cargo clippy --target x86_64-unknown-linux-gnu \
        --no-default-features --features native-sdk -- -D warnings
    PATH="$rustup_bin:$PATH" cargo clippy --target x86_64-pc-windows-msvc \
        --no-default-features --features native-sdk -- -D warnings
fi

# ---- gated tests in a real Linux VM (Apple `container` CLI) --------
# Compiles AND runs the unit tests inside the OS-gated modules — the
# one class of breakage nothing above can reach (a missed struct field
# in a native_sdk #[cfg(test)] literal shipped red to CI exactly this
# way). Skipped automatically when the container tooling isn't running.
# NOTE: the VM's virtiofs mount chokes on iCloud-synced paths
# (~/Documents ⇒ EDEADLK), hence the rsync to /private/tmp first.
if ! skipped vm; then
    if container system status >/dev/null 2>&1; then
        step "CI ubuntu legs in Linux VM: clippy --workspace --all-targets + cargo test, stable ($STABLE_FEATURES) and $NIGHTLY_TOOLCHAIN (--all-features)"
        # Dedicated directory: nothing else may write here (a shared
        # scratch copy once had another tool rsync over it mid-run).
        vm_dir=/private/tmp/aaronia-ci-vm
        rsync -a --delete --exclude target --exclude target-linux \
            --exclude .git --exclude .claude ./ "$vm_dir/"
        # The full --all-targets matters: an OS-gated *example* with a
        # stale call signature shipped red to CI past a --lib-only VM
        # run. This mirrors CI's ubuntu `test` and `nightly` legs
        # command-for-command (the nightly leg is where SdkSinkBlock,
        # futuresdr + native-sdk, compiles). The pinned nightly is
        # installed per run too: --rm discards it with the container.
        # rust:slim lacks what the GitHub ubuntu runner image ships
        # preinstalled: clippy, pkg-config, ALSA headers (the
        # `futuresdr-audio` feature -> alsa-sys), and python3 (pyo3's build script
        # needs an interpreter for the --workspace clippy leg). Install
        # per run; the persistent target/ under $vm_dir keeps rebuilds
        # incremental.
        # --memory 8g: the all-features lib-test link OOM-kills the
        # container CLI's default allocation (ld dies with SIGKILL).
        # clippy is --workspace (mirrors CI); test stays root-crate —
        # python-aaronia's Python-side behavior is covered by the
        # python step below and CI's maturin+pytest job.
        container run --rm --memory 8g --cpus 4 \
            -v "$vm_dir":/w -w /w -e PROPTEST_CASES="$PROPTEST_CASES" \
            -e STABLE_FEATURES="$STABLE_FEATURES" -e NIGHTLY_TOOLCHAIN="$NIGHTLY_TOOLCHAIN" rust:slim \
            sh -c 'rustup component add clippy >/dev/null 2>&1; \
                   rustup toolchain install "$NIGHTLY_TOOLCHAIN" --profile minimal \
                       --component clippy >/dev/null 2>&1; \
                   apt-get update -qq >/dev/null && \
                   apt-get install -y -qq pkg-config libasound2-dev python3 >/dev/null; \
                   cargo clippy --workspace --features "$STABLE_FEATURES" --all-targets -- -D warnings \
                   && cargo test --features "$STABLE_FEATURES" \
                   && cargo +"$NIGHTLY_TOOLCHAIN" clippy --workspace --all-features --all-targets -- -D warnings \
                   && cargo +"$NIGHTLY_TOOLCHAIN" test --all-features'
    else
        echo "vm step: 'container system start' not running — skipping" \
             "(gated-module tests only compile on CI's linux/windows legs)" >&2
    fi
fi

# ---- python job (maturin build + pytest, mirrors ci.yml) -----------
if ! skipped python; then
    if command -v python3 >/dev/null 2>&1; then
        step "python bindings: cargo check -p python-aaronia (+ pytest when venv exists)"
        cargo check -p python-aaronia
        # Full maturin build + pytest when the dev venv is set up
        # (python-aaronia/venv with maturin installed); cargo check
        # alone still catches compile breakage everywhere.
        if [[ -x python-aaronia/venv/bin/maturin ]]; then
            (cd python-aaronia \
                && VIRTUAL_ENV=$PWD/venv PATH=$PWD/venv/bin:$PATH \
                   maturin develop --release >/dev/null \
                && ./venv/bin/python -m pytest test_basic.py -q)
        else
            echo "python step: no python-aaronia/venv with maturin — ran cargo check only" >&2
        fi
    else
        echo "python step: python3 not found — skipping" >&2
    fi
fi

# ---- soapy job (cmake build + load check, mirrors ci.yml) ----------
if ! skipped soapy; then
    if pkg-config --exists SoapySDR 2>/dev/null; then
        step "soapy plugin: cmake build + SoapySDRUtil --check=aaronia"
        cmake -S soapy-aaronia -B soapy-aaronia/build -DCMAKE_BUILD_TYPE=Release >/dev/null
        cmake --build soapy-aaronia/build >/dev/null
        SOAPY_SDR_PLUGIN_PATH=$PWD/soapy-aaronia/build SoapySDRUtil --check=aaronia
    else
        echo "soapy step: SoapySDR not installed (brew install soapysdr / apt libsoapysdr-dev) — skipping" >&2
    fi
fi

printf '\n\033[1;32mci-local: all steps passed.\033[0m\n'
