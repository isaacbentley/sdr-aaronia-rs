# SoapySDR plugin module for Aaronia SPECTRAN V6, backed by the
# sdr-aaronia-rs Rust crate's C API.
#
# The Rust *static* library is built by cargo as part of this build
# (add_custom_target below) and linked statically into the module —
# linking the cdylib instead (what a bare `find_library` picks first)
# ships a module with an absolute-path dependency on a build-machine
# .so/.dylib that no release artifact contains, and the module can
# never load on user machines.
cmake_minimum_required(VERSION 3.14)
project(SoapyAaronia CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(SoapySDR "0.7" REQUIRED)

# SoapySDR's installed CMake export lists `-flat_namespace` in the
# imported target's INTERFACE_LINK_LIBRARIES, so it lands at the end of
# every module's link line and forces flat-namespace symbol binding.
# That is how a module could end up with a dangling bind: the linker
# defined a Rust vtable symbol, localised it into this image, and still
# emitted a flat-namespace bind for it, which dyld could not resolve —
# so `dlopen` failed with "symbol not found in flat namespace" and the
# published macOS module never loaded. The module links directly
# against libSoapySDR, so two-level binding resolves those symbols
# without it.
#
# Also drop `-undefined dynamic_lookup`, which CMake has spelled both
# ways across versions and recent versions omit entirely: a module that
# fails to load is worse than one that fails to link, which is the
# reason SoapySDR's own macro passes `-Wl,--no-undefined` for GNU
# toolchains.
if (APPLE AND TARGET SoapySDR)
    get_target_property(_soapy_iface SoapySDR INTERFACE_LINK_LIBRARIES)
    if (_soapy_iface)
        list(REMOVE_ITEM _soapy_iface "-flat_namespace")
        set_target_properties(SoapySDR PROPERTIES
            INTERFACE_LINK_LIBRARIES "${_soapy_iface}")
    endif()
endif()

if (APPLE)
    string(REGEX REPLACE "(-Wl,)?-undefined[ ,]dynamic_lookup" ""
        CMAKE_SHARED_MODULE_CREATE_CXX_FLAGS
        "${CMAKE_SHARED_MODULE_CREATE_CXX_FLAGS}")
endif()

# Include path to aaronia.h
include_directories(${CMAKE_CURRENT_SOURCE_DIR}/../include)

# --- Rust static library -------------------------------------------------
set(CARGO_MANIFEST_DIR ${CMAKE_CURRENT_SOURCE_DIR}/..)
if (NOT DEFINED CARGO_TARGET_DIR)
    set(CARGO_TARGET_DIR ${CARGO_MANIFEST_DIR}/target)
endif()

if (WIN32)
    set(RUST_STATIC_LIB ${CARGO_TARGET_DIR}/release/sdr_aaronia_rs.lib)
else()
    set(RUST_STATIC_LIB ${CARGO_TARGET_DIR}/release/libsdr_aaronia_rs.a)
endif()

# Build (or refresh) the static lib with the FFI feature set. -p scopes
# to the root crate so the workspace's python-aaronia member (a pyo3
# extension) is not dragged into a C++ plugin build.
add_custom_command(
    OUTPUT ${RUST_STATIC_LIB}
    COMMAND cargo build --release -p sdr-aaronia-rs
    WORKING_DIRECTORY ${CARGO_MANIFEST_DIR}
    COMMENT "cargo build --release -p sdr-aaronia-rs (static lib for SoapyAaronia)"
    VERBATIM
    # No DEPENDS: cargo is its own dependency tracker; the command
    # reruns each build and no-ops when the crate is up to date.
)
add_custom_target(sdr_aaronia_rs_static ALL DEPENDS ${RUST_STATIC_LIB})

# Source files
set(AaroniaSupport_SOURCES
    AaroniaSoapyDevice.cpp
    Registration.cpp
)

# Build SoapySDR module
SOAPY_SDR_MODULE_UTIL(
    TARGET aaroniaSupport
    SOURCES ${AaroniaSupport_SOURCES}
    LIBRARIES ${RUST_STATIC_LIB}
)
add_dependencies(aaroniaSupport sdr_aaronia_rs_static)

if (APPLE)
    # Belt and braces with the flag removal above: whichever `-undefined`
    # the generator emits, this one comes last and wins.
    target_link_options(aaroniaSupport PRIVATE "LINKER:-undefined,error")

    # `-flat_namespace` is stripped above, so the linker cannot emit a
    # flat-namespace bind for a symbol it defined here. The module load
    # check in CI is what proves that, on the toolchain that shipped the
    # broken module.
endif()

# System libraries required by the statically-linked Rust runtime
# (std, tokio, reqwest/rustls/aws-lc). Missing entries here surface as
# unresolved symbols at module link time.
if (APPLE)
    target_link_libraries(aaroniaSupport PRIVATE
        "-framework Security"
        "-framework CoreFoundation"
        "-framework SystemConfiguration"
    )
elseif (WIN32)
    target_link_libraries(aaroniaSupport PRIVATE
        ws2_32 bcrypt ntdll userenv crypt32 secur32 advapi32
    )
else()
    target_link_libraries(aaroniaSupport PRIVATE pthread dl m)
endif()
