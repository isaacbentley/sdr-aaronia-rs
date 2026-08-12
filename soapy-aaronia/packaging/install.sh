#!/usr/bin/env sh
# Install the Aaronia SoapySDR module next to this script.
#
# Finds where SoapySDR loads modules from, copies the plugin there, and
# clears the macOS quarantine flag that stops a downloaded binary from
# loading. Falls back to printing what to do by hand if anything is
# missing, since a wrong guess here is worse than an instruction.
set -eu

dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
module=$(ls "$dir"/libaaroniaSupport.so "$dir"/aaroniaSupport.dll 2>/dev/null | head -1 || true)

if [ -z "$module" ]; then
    echo "No plugin found next to this script." >&2
    echo "Run it from the unpacked release archive." >&2
    exit 1
fi

# macOS quarantines anything downloaded; clear it before the copy so the
# installed file is clean too.
if [ "$(uname -s)" = "Darwin" ]; then
    xattr -d com.apple.quarantine "$module" 2>/dev/null || true
fi

if ! command -v SoapySDRUtil >/dev/null 2>&1; then
    echo "SoapySDRUtil is not on PATH, so the module directory cannot be found."
    echo
    echo "Install SoapySDR, or use the plugin without installing:"
    echo "    export SOAPY_SDR_PLUGIN_PATH=$dir"
    echo "    SoapySDRUtil --check=aaronia"
    exit 1
fi

target=$(SoapySDRUtil --info 2>/dev/null | sed -n 's/^Search path:[[:space:]]*//p' | head -1)
if [ -z "$target" ]; then
    # Fall back to the directory of an already-installed module. Strip
    # the filename with parameter expansion rather than `xargs dirname`,
    # which splits on whitespace and would mangle a path containing a
    # space.
    # SoapySDRUtil appends " (version)" to each module line; drop it,
    # then take the directory.
    found=$(SoapySDRUtil --info 2>/dev/null \
        | sed -n 's/^Module found:[[:space:]]*//p' \
        | head -1 \
        | sed 's/[[:space:]]*([^)]*)$//')
    [ -n "$found" ] && target=${found%/*}
fi

if [ -z "$target" ] || [ ! -d "$target" ]; then
    echo "Could not determine SoapySDR's module directory from SoapySDRUtil --info."
    echo
    echo "Use the plugin without installing:"
    echo "    export SOAPY_SDR_PLUGIN_PATH=$dir"
    echo "    SoapySDRUtil --check=aaronia"
    exit 1
fi

echo "Installing $(basename "$module") into $target"
if [ -w "$target" ]; then
    cp "$module" "$target/"
else
    # Say exactly what is about to run as root before the password
    # prompt appears, so nothing is buried inside a downloaded script.
    echo "That directory is not writable. Running:"
    echo "    sudo cp \"$module\" \"$target/\""
    sudo cp "$module" "$target/"
fi

echo
echo "Checking the module loads:"
SoapySDRUtil --check=aaronia
echo
echo "Done. Try your device with:"
echo "    SoapySDRUtil --probe=\"driver=aaronia,url=http://localhost:54664\""
