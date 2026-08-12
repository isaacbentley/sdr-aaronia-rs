//! Error types for sdr-aaronia-rs

/// Central error type for the crate.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// HTTP status error from the RTSA endpoint.
    #[cfg(feature = "http")]
    #[error("HTTP {status}: {context}")]
    Http {
        /// HTTP status code returned by the server.
        status: reqwest::StatusCode,
        /// Human-readable description of the failure context.
        context: String,
    },

    /// Underlying transport/network error (e.g. connection refused, timeout).
    #[cfg(feature = "http")]
    #[error(transparent)]
    Transport(#[from] reqwest::Error),

    /// Stream protocol parsing or formatting error.
    #[error("stream protocol error: {0}")]
    Protocol(String),

    /// The sample stream ended and will produce no more data: the
    /// server closed it, or auto-reconnect gave up.
    ///
    /// Distinct from [`Error::Protocol`] because a consumer looping
    /// over blocks has to tell "there is no more data" apart from "a
    /// read failed", and the two demand opposite responses. Reported
    /// once the reader task has stopped.
    #[error("sample stream closed: {0}")]
    StreamClosed(String),

    /// RTSA capture file format or parsing error.
    #[error("RTSA file format error at 0x{offset:08X}: {reason}")]
    FileFormat {
        /// Byte offset within the file where the error was detected.
        offset: u64,
        /// Description of the format violation.
        reason: String,
    },

    /// Native SDK library not found or could not be loaded.
    #[error("Aaronia SDK not installed")]
    SdkNotInstalled,

    /// Native SDK loading error (libloading).
    #[cfg(feature = "native-sdk")]
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[error(transparent)]
    SdkLoader(#[from] libloading::Error),

    /// Native SDK operation failed with a specific API error code.
    #[cfg(feature = "native-sdk")]
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[error("SDK error during {operation}: {code}")]
    SdkApi {
        /// The operation that failed (e.g. "AARTSAAPI_Init").
        operation: String,
        /// The underlying granular SDK error code.
        #[source]
        code: crate::native_sdk::SdkError,
    },

    /// Native SDK operation failed.
    #[error("SDK error: {0}")]
    Sdk(String),

    /// Remote config feature requires a license.
    #[error("Remote Config licence required")]
    NotLicensed,

    /// Standard I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Invalid configuration parameters (e.g. invalid frequency).
    #[error("Configuration error: {0}")]
    Config(String),

    /// Component failed to initialize.
    #[error("Initialization error: {0}")]
    Initialization(String),
}

/// A specialized `Result` type for `sdr-aaronia-rs` operations.
pub type Result<T, E = Error> = std::result::Result<T, E>;
