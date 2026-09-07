#!/usr/bin/env sh
# Load the real Aaronia RTSA SDK library and run the crate's native-SDK
# load test against it, in an x86-64 Linux container — no hardware, and
# no x86-64 Linux host, needed.
#
# The SDK is proprietary and cannot live in this repository, so point
# this at an unpacked RTSA-Suite PRO Linux archive. Uses Apple's
# `container` CLI (macOS 26+); the same recipe works with Docker by
# swapping the command name and --mount syntax.
#
# What it proves: the library's dependencies resolve, `AARTSAAPI_Version`
# answers, and `detection::get_sdk_library_path` finds the library at the
# default Linux install path with no environment variable — the layout
# RTSA-Suite 3.0.3 ships (library in the install root, not `sdk/`).
#
#   scripts/sdk-container-test.sh ~/Downloads/aaronia-rtsa-suite-3.0.3.16655-Linux/opt/aaronia-rtsa-suite
set -eu

root=${1:?usage: $0 <path to unpacked .../opt/aaronia-rtsa-suite>}
[ -f "$root/Aaronia-RTSA-Suite-PRO/libAaroniaRTSAAPI.so" ] || {
    echo "no libAaroniaRTSAAPI.so under $root/Aaronia-RTSA-Suite-PRO" >&2
    exit 1
}
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

# The library's RUNPATH is $ORIGIN:$ORIGIN/../lib, so the Qt and HDF5 it
# ships in <root>/lib resolve on their own once the whole tree is
# mounted at its install path. What the bundled Qt then needs from the
# system is these nine packages.
deps="libusb-1.0-0 libgl1 libglx0 libopengl0 libegl1 libxkbcommon0 libpulse0 libglib2.0-0 libdbus-1-3"

exec container run --rm --arch amd64 \
    --mount type=bind,source="$root",target=/opt/aaronia-rtsa-suite,readonly \
    --mount type=bind,source="$repo",target=/src \
    rust:bookworm sh -c "
        (apt-get update -qq && apt-get install -y -qq $deps) >/dev/null 2>&1
        cd /src
        export CARGO_TARGET_DIR=/tmp/target
        cargo test --no-default-features --features native-sdk,file \
            --test native_sdk_load -- --ignored --nocapture
    "
