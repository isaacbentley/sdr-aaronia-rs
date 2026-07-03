//! Aaronia RTSA SDK / RTSA-Suite installation detection.
//!
//! Resolution order for the installation directory:
//! 1. The `AARONIA_SDK_PATH` environment variable, if set to an existing
//!    directory. This works on every platform and covers containers,
//!    custom prefixes, and macOS (where there is no fixed default path).
//! 2. The platform default install path from the official SDK
//!    documentation (Windows and Linux only).
//!
//! **Native SDK support is Windows/Linux only, full stop.** Aaronia
//! publishes SDK binaries for exactly those two platforms; this crate's
//! FFI bindings ([`crate::native_sdk`], [`crate::sdk_source`]) are gated
//! to `any(target_os = "windows", target_os = "linux")` at the crate
//! root regardless of the `native-sdk` feature flag's own state. The
//! functions in this module are deliberately callable on *every*
//! platform (so portable code can check `is_sdk_installed()` without
//! its own `#[cfg]`), but on macOS and any other target they always
//! return `None`/`false` — see [`sdk_library_name`] for why that isn't
//! just "no install found on this machine" but "this platform has no
//! SDK to find".

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
///
/// Windows and Linux only: Aaronia has never published a macOS build of
/// the native SDK (only `win64`/`linux64` binaries exist), and this
/// crate's FFI bindings (`native_sdk`, `sdk_source`) are themselves
/// gated to `any(target_os = "windows", target_os = "linux")` at the
/// crate root — there is no code path anywhere that could load an SDK
/// library on macOS even if one existed. An earlier revision guessed a
/// `.dylib` name here, which made [`get_sdk_library_path`] (and hence
/// [`is_sdk_installed`], both callable unconditionally on every
/// platform) able to report `Some`/`true` on macOS for a library this
/// crate has no way to actually use.
fn sdk_library_name() -> Option<&'static str> {
    #[cfg(target_os = "windows")]
    return Some("AaroniaRTSAAPI.dll");
    #[cfg(target_os = "linux")]
    return Some("libAaroniaRTSAAPI.so");
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Process-wide env-var manipulation makes these tests inherently
    /// non-parallel-safe (same rationale as `utils::test_user_agent_*`);
    /// restore-on-drop keeps state from bleeding into other tests.
    struct EnvGuard {
        original: Option<String>,
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.original.take() {
                Some(v) => unsafe { std::env::set_var(SDK_PATH_ENV, v) },
                None => unsafe { std::env::remove_var(SDK_PATH_ENV) },
            }
        }
    }
    fn guard() -> EnvGuard {
        EnvGuard {
            original: std::env::var(SDK_PATH_ENV).ok(),
        }
    }

    /// Minimal self-cleaning temp directory. `detection` is compiled
    /// unconditionally (no feature gate), so its tests can't depend on
    /// the crate's own optional `tempfile` dependency, which is only
    /// pulled in by the `file` feature — `cargo test --no-default-features
    /// --features native-sdk` would fail to find it otherwise.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(label: &str) -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "sdr-aaronia-rs-detection-test-{label}-{}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// On any platform other than Windows/Linux, there is genuinely no
    /// SDK library to find — Aaronia doesn't publish one, and this
    /// crate's `native_sdk`/`sdk_source` FFI modules don't exist there
    /// either. Guards against a future edit accidentally reintroducing a
    /// speculative filename (e.g. a `.dylib` guess) for a platform with
    /// no real SDK build and no bindings to load it.
    #[test]
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    fn sdk_library_name_is_none_off_windows_linux() {
        assert!(
            sdk_library_name().is_none(),
            "no platform other than Windows/Linux should claim an SDK library name"
        );
    }

    #[test]
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn sdk_library_name_is_some_on_windows_linux() {
        assert!(sdk_library_name().is_some());
    }

    /// All `AARONIA_SDK_PATH`-dependent assertions live in *one* test
    /// function, sequentially, rather than split across separate
    /// `#[test]`s. Rust's default harness runs tests in parallel threads
    /// sharing one process environment — three separate tests each
    /// mutating the same env var raced against each other (each one
    /// could observe or clobber another's in-flight value) the first
    /// time this was tried. Same rationale, same fix shape as
    /// `utils::test_user_agent_default_and_override`.
    #[test]
    fn sdk_path_env_var_resolution() {
        let _g = guard();

        // 1. No env var set: is_sdk_installed() must be callable (and
        // return an answer, not panic) on every platform, whatever the
        // real answer is on this machine — it's re-exported
        // unconditionally at the crate root.
        unsafe { std::env::remove_var(SDK_PATH_ENV) };
        let _ = is_sdk_installed();

        // 2. Env var pointing at a real directory that doesn't contain
        // `sdk/<library>` must resolve to "not installed" — a false
        // positive from the directory merely existing (e.g. because a
        // real SDK happens to be installed at the platform default path
        // on this machine) would defeat the whole point of the override.
        let empty_dir = TempDir::new("empty");
        unsafe { std::env::set_var(SDK_PATH_ENV, empty_dir.path()) };
        assert_eq!(
            get_sdk_path().as_deref(),
            Some(empty_dir.path().to_str().unwrap()),
            "the env var must be honoured even though it has no library in it"
        );
        assert_eq!(get_sdk_library_path(), None);
        assert!(!is_sdk_installed());

        // 3. On Windows/Linux, placing a stand-in library file at
        // `<AARONIA_SDK_PATH>/sdk/<expected name>` must be discovered —
        // this is the layout the real installer produces (verified
        // against the official win64/linux64 SDK distributions).
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            let lib_dir = TempDir::new("with-lib");
            let sdk_subdir = lib_dir.path().join("sdk");
            std::fs::create_dir_all(&sdk_subdir).unwrap();
            let lib_name = sdk_library_name().expect("Windows/Linux always has a library name");
            std::fs::write(
                sdk_subdir.join(lib_name),
                b"not a real library, just a stand-in",
            )
            .unwrap();

            unsafe { std::env::set_var(SDK_PATH_ENV, lib_dir.path()) };

            let found = get_sdk_library_path().expect("library must be discovered");
            assert!(found.ends_with(lib_name));
            assert!(is_sdk_installed());
        }
    }
}
