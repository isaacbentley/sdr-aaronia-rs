//! Aaronia RTSA SDK / RTSA-Suite installation detection.
//!
//! Resolution order for the installation directory:
//! 1. The `AARONIA_SDK_PATH` environment variable, if set to an existing
//!    directory. This works on every platform and covers containers,
//!    custom prefixes, and macOS (where there is no fixed default path).
//! 2. The platform default install path from the official SDK
//!    documentation (Windows and Linux only).

use std::path::{Path, PathBuf};

/// Environment variable overriding the RTSA-Suite installation directory.
pub const SDK_PATH_ENV: &str = "AARONIA_SDK_PATH";

/// Detects if the Aaronia RTSA SDK is installed using verified installation paths
/// from official SDK documentation
pub fn is_sdk_installed() -> bool {
    get_sdk_library_path().is_some()
}

/// Returns the SDK installation path if detected.
///
/// Checks [`SDK_PATH_ENV`] first (any platform), then the documented
/// per-platform default install locations.
pub fn get_sdk_path() -> Option<String> {
    if let Ok(env_path) = std::env::var(SDK_PATH_ENV) {
        let trimmed = env_path.trim();
        if !trimmed.is_empty() {
            if Path::new(trimmed).is_dir() {
                return Some(trimmed.to_string());
            }
            tracing::warn!(
                "{} is set to {:?} but that directory does not exist; \
                 falling back to the default install path",
                SDK_PATH_ENV,
                trimmed
            );
        }
    }

    default_sdk_path()
}

/// Platform-default installation directory, if it exists.
fn default_sdk_path() -> Option<String> {
    #[cfg(target_os = "windows")]
    let candidate = Some("C:\\Program Files\\Aaronia AG\\Aaronia RTSA-Suite PRO");

    #[cfg(target_os = "linux")]
    let candidate = Some("/opt/aaronia-rtsa-suite/Aaronia-RTSA-Suite-PRO");

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    let candidate: Option<&str> = None;

    candidate
        .filter(|p| Path::new(p).exists())
        .map(str::to_string)
}

/// Platform-specific SDK library filename.
fn sdk_library_name() -> Option<&'static str> {
    #[cfg(target_os = "windows")]
    return Some("AaroniaRTSAAPI.dll");
    #[cfg(target_os = "linux")]
    return Some("libAaroniaRTSAAPI.so");
    #[cfg(target_os = "macos")]
    return Some("libAaroniaRTSAAPI.dylib");
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    None
}

/// Returns the path to the SDK library file
pub fn get_sdk_library_path() -> Option<String> {
    let sdk_path = get_sdk_path()?;
    let lib_name = sdk_library_name()?;
    let lib_path: PathBuf = [sdk_path.as_str(), "sdk", lib_name].iter().collect();
    if lib_path.exists() {
        Some(lib_path.to_string_lossy().into_owned())
    } else {
        None
    }
}

/// Returns the XML configuration directory path
pub fn get_xml_config_path() -> Option<String> {
    // XML files live in the main installation directory on every platform.
    get_sdk_path()
}

/// Returns the path to the RTSAFileTool CLI utility
pub fn get_rtsa_file_tool_path() -> Option<String> {
    let sdk_path = get_sdk_path()?;

    #[cfg(target_os = "windows")]
    let tool_name = "RTSAFileTool.exe";
    #[cfg(not(target_os = "windows"))]
    let tool_name = "RTSAFileTool";

    let tool_path: PathBuf = [sdk_path.as_str(), tool_name].iter().collect();
    if tool_path.exists() {
        Some(tool_path.to_string_lossy().into_owned())
    } else {
        None
    }
}
