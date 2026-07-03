//! Native-SDK library-loading smoke test.
//!
//! Verifies the FFI layer against the real `libAaroniaRTSAAPI` binary
//! without hardware: loading the shared library resolves every one of the
//! 32 bound symbols (a missing or renamed export fails here), and calling
//! `AARTSAAPI_Version()` proves calls actually go through a resolved
//! function pointer with the right ABI.
//!
//! Requires the SDK library on disk. Point `AARONIA_SDK_PATH` at a
//! directory containing `sdk/libAaroniaRTSAAPI.so` (Linux) /
//! `sdk\AaroniaRTSAAPI.dll` (Windows), or install RTSA-Suite PRO at the
//! default path. Run with:
//!
//! ```sh
//! cargo test --features native-sdk --test native_sdk_load -- --ignored --nocapture
//! ```
#![cfg(all(
    feature = "native-sdk",
    any(target_os = "windows", target_os = "linux")
))]

#[test]
#[ignore = "requires the Aaronia SDK library (set AARONIA_SDK_PATH)"]
fn native_sdk_library_loads_and_resolves_all_symbols() {
    use sdr_aaronia_rs::native_sdk::NativeSdkClient;

    assert!(
        sdr_aaronia_rs::is_sdk_installed(),
        "SDK not detected — set AARONIA_SDK_PATH to an install dir containing \
         sdk/libAaroniaRTSAAPI.so"
    );

    // `new` dlopens the library and resolves all 32 function pointers;
    // any ABI-name drift fails right here.
    let client =
        unsafe { NativeSdkClient::new() }.expect("library must load and all symbols must resolve");

    // A real call through a resolved pointer: Version() needs no Init and
    // returns `major << 16 | minor`.
    let version = unsafe { client.get_version() };
    println!(
        "AARTSAAPI_Version() = 0x{version:08x} (v{}.{})",
        version >> 16,
        version & 0xFFFF
    );
    assert!(version > 0, "Version() should report a non-zero version");
}
