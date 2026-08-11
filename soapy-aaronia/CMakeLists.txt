cmake_minimum_required(VERSION 3.14)
project(SoapyAaronia CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(SoapySDR "0.7" REQUIRED)

# Include path to aaronia.h
include_directories(${CMAKE_CURRENT_SOURCE_DIR}/../include)

# Source files
set(AaroniaSupport_SOURCES
    AaroniaSoapyDevice.cpp
    Registration.cpp
)

# Find Rust static library artifact
find_library(SDR_AARONIA_RS_LIB
    NAMES sdr_aaronia_rs libsdr_aaronia_rs.a sdr_aaronia_rs.lib
    HINTS
        ${CMAKE_CURRENT_SOURCE_DIR}/../target/release
        ${CMAKE_CURRENT_SOURCE_DIR}/../target/debug
)

if (NOT SDR_AARONIA_RS_LIB)
    message(WARNING "sdr-aaronia-rs Rust library not found! Run 'cargo build --release' in the parent directory first.")
endif()

# Build SoapySDR module
SOAPY_SDR_MODULE_UTIL(
    TARGET aaroniaSupport
    SOURCES ${AaroniaSupport_SOURCES}
    LIBRARIES ${SDR_AARONIA_RS_LIB}
)

if (APPLE)
    target_link_libraries(aaroniaSupport PRIVATE "-framework Security" "-framework CoreFoundation")
elseif (UNIX)
    target_link_libraries(aaroniaSupport PRIVATE pthread dl m)
endif()
