#!/usr/bin/env bash
#
# Build sdr-aaronia-rs as a cdylib with AddressSanitizer + UBSAN, link the
# C smoke test, and run it. Any sanitizer hit aborts and the script
# exits non-zero.
#
# Pre-reqs (CI installs these):
#   * nightly Rust toolchain
#   * rust-src component (so `-Z build-std` can re-build stdlib with
#     sanitizer instrumentation)
#   * a C compiler that accepts `-fsanitize=address,undefined` (gcc or
#     clang on Linux; xcrun clang on macOS)
#
# Run locally:
#   bash tests/asan/run_asan.sh
#
# The script picks `x86_64-unknown-linux-gnu` on Linux and bails on
# other hosts because ASAN coverage of the Rust cdylib is Linux-only
# in stable nightlies.

set -euo pipefail

OS="$(uname -s)"
if [[ "$OS" != "Linux" ]]; then
    echo "run_asan.sh: only Linux is supported (got $OS). Skipping." >&2
    exit 0
fi

# Stop on the first sanitizer hit so the failure is visible in CI logs.
export ASAN_OPTIONS="${ASAN_OPTIONS:-detect_leaks=0:abort_on_error=1:halt_on_error=1}"
export UBSAN_OPTIONS="${UBSAN_OPTIONS:-halt_on_error=1:print_stacktrace=1}"

TARGET="x86_64-unknown-linux-gnu"
PROFILE="debug"

# `-Z sanitizer=address` instruments the cdylib at compile time.
# `-Z build-std` rebuilds the standard library with the same flags so
# UB inside std (e.g. allocator code paths) is also caught.
export RUSTFLAGS="-Z sanitizer=address"

echo "==> cargo +nightly build --target $TARGET (ASAN-instrumented)"
cargo +nightly build \
    --target "$TARGET" \
    -Z build-std=panic_abort,std

LIB_DIR="target/$TARGET/$PROFILE"

# `-fsanitize=address,undefined` on the C side so the linker brings in
# the same ASAN runtime that the Rust side expects.
CC="${CC:-cc}"
SMOKE_BIN="$LIB_DIR/c_smoke"

echo "==> $CC c_smoke (ASAN+UBSAN)"
"$CC" -O1 -g \
    -fsanitize=address,undefined \
    -fno-omit-frame-pointer \
    -Iinclude \
    -o "$SMOKE_BIN" tests/asan/c_smoke.c \
    -L"$LIB_DIR" -lsdr_aaronia_rs \
    -Wl,-rpath,"$LIB_DIR"

echo "==> running $SMOKE_BIN"
LD_LIBRARY_PATH="$LIB_DIR" "$SMOKE_BIN"
