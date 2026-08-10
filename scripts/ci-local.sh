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
# Usage:
#   scripts/ci-local.sh            # run everything (recommended pre-push)
#   SKIP="miri hack" scripts/ci-local.sh   # skip named steps
#
# Tools beyond stable cargo: cargo-deny, cargo-hack, cargo-machete, and
# the pinned miri nightly. The preflight check below prints the install
# command for anything missing.

set -euo pipefail
cd "$(dirname "$0")/.."

# Property tests: same case count CI uses.
export PROPTEST_CASES="${PROPTEST_CASES:-256}"

# Pinned to match the miri job in ci.yml (see the comment there about
# the futuredsp `fadd_fast` breakage on current nightlies). Bump both
# together.
MIRI_TOOLCHAIN=nightly-2026-01-01

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

require deny    cargo deny --version           -- "cargo install cargo-deny"
require hack    cargo hack --version           -- "cargo install cargo-hack"
require machete cargo machete --version        -- "cargo install cargo-machete"
require miri    env PATH="$(miri_bin):$PATH" cargo miri --version \
        -- "rustup toolchain install $MIRI_TOOLCHAIN --component miri"
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
    step "cargo clippy --all-features --all-targets -- -D warnings"
    cargo clippy --all-features --all-targets -- -D warnings
fi

if ! skipped test; then
    # CI's Linux leg wraps this in cargo-llvm-cov for coverage; the
    # test outcome is identical, so plain `cargo test` suffices here.
    step "cargo test --all-features  (PROPTEST_CASES=$PROPTEST_CASES)"
    cargo test --all-features
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
    step "cargo hack check --each-feature --no-dev-deps"
    cargo hack check --each-feature --no-dev-deps
fi

# ---- machete job ---------------------------------------------------
if ! skipped machete; then
    step "cargo machete"
    cargo machete
fi

# ---- miri job (FFI-free module allowlist, pinned nightly) ----------
if ! skipped miri; then
    step "cargo miri test (decompression, http_streaming, utils) on $MIRI_TOOLCHAIN"
    PATH="$(miri_bin):$PATH" cargo miri test --lib decompression
    PATH="$(miri_bin):$PATH" cargo miri test --lib http_streaming
    PATH="$(miri_bin):$PATH" cargo miri test --lib utils
fi

printf '\n\033[1;32mci-local: all steps passed.\033[0m\n'
