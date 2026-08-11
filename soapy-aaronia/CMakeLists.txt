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
