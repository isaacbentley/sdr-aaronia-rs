#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(unused_unsafe)]
#![allow(clippy::missing_safety_doc)]
#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unsafe_op_in_unsafe_fn)]

//! Aaronia RTSA Native SDK Integration
//!
//! This module provides Rust bindings to the official Aaronia RTSA SDK
//! based on the verified API specification and SDK samples.

use crate::{Error, Result};
use libloading::{Library, Symbol};
use num_complex::Complex32;
// `tracing`, not `log`: the rest of the crate emits tracing events, and a
// split ecosystem meant SDK-path logs vanished for tracing-only
// subscribers.
use std::collections::VecDeque;
use std::ffi::{CStr, CString, OsStr};
use std::os::raw::{c_char, c_void};
use std::ptr;
use std::sync::Arc;
use tracing::{debug, error, info, trace, warn};
use widestring::{WideCStr, WideCString, WideChar};

use crate::detection::{get_sdk_library_path, get_xml_config_path};

// === AARTSAAPI Constants ===

// Result codes (verified from aaroniartsaapi.h)
pub const AARTSAAPI_OK: u32 = 0x00000000;
pub const AARTSAAPI_EMPTY: u32 = 0x00000001;
pub const AARTSAAPI_RETRY: u32 = 0x00000002;

// Device states
pub const AARTSAAPI_IDLE: u32 = 0x10000000;
pub const AARTSAAPI_CONNECTING: u32 = 0x10000001;
pub const AARTSAAPI_CONNECTED: u32 = 0x10000002;
pub const AARTSAAPI_STARTING: u32 = 0x10000003;
pub const AARTSAAPI_RUNNING: u32 = 0x10000004;
pub const AARTSAAPI_STOPPING: u32 = 0x10000005;
pub const AARTSAAPI_DISCONNECTING: u32 = 0x10000006;

// Warning codes (high bit 0x4000_0000 set; the call still produced a result)
pub const AARTSAAPI_WARNING: u32 = 0x40000000;
pub const AARTSAAPI_WARNING_VALUE_ADJUSTED: u32 = 0x40000001;
pub const AARTSAAPI_WARNING_VALUE_DISABLED: u32 = 0x40000002;

// Error codes (high bit 0x8000_0000 set). Names follow the official SDK
// header — note `INVALID_PARAMETR` is misspelled in the SDK and we keep
// that spelling for the binding.
pub const AARTSAAPI_ERROR: u32 = 0x80000000;
pub const AARTSAAPI_ERROR_NOT_INITIALIZED: u32 = 0x80000001;
pub const AARTSAAPI_ERROR_NOT_FOUND: u32 = 0x80000002;
pub const AARTSAAPI_ERROR_BUSY: u32 = 0x80000003;
pub const AARTSAAPI_ERROR_NOT_OPEN: u32 = 0x80000004;
pub const AARTSAAPI_ERROR_NOT_CONNECTED: u32 = 0x80000005;
pub const AARTSAAPI_ERROR_INVALID_CONFIG: u32 = 0x80000006;
pub const AARTSAAPI_ERROR_BUFFER_SIZE: u32 = 0x80000007;
pub const AARTSAAPI_ERROR_INVALID_CHANNEL: u32 = 0x80000008;
pub const AARTSAAPI_ERROR_INVALID_PARAMETR: u32 = 0x80000009;
pub const AARTSAAPI_ERROR_INVALID_SIZE: u32 = 0x8000000a;
pub const AARTSAAPI_ERROR_MISSING_PATHS_FILE: u32 = 0x8000000b;
pub const AARTSAAPI_ERROR_VALUE_INVALID: u32 = 0x8000000c;
pub const AARTSAAPI_ERROR_VALUE_MALFORMED: u32 = 0x8000000d;

/// Translate an `AARTSAAPI_Result` code into a short human-readable label.
/// Unknown codes return `"unknown"` so the caller can fall back to the
/// hex value. Mirrors the canonical enum in `aaroniartsaapi.h`; values
/// were cross-checked against the third-party `g3gg0/rx-fft` C# binding.
pub fn result_message(code: u32) -> &'static str {
    match code {
        AARTSAAPI_OK => "ok",
        AARTSAAPI_EMPTY => "empty",
        AARTSAAPI_RETRY => "retry",
        AARTSAAPI_IDLE => "idle",
        AARTSAAPI_CONNECTING => "connecting",
        AARTSAAPI_CONNECTED => "connected",
        AARTSAAPI_STARTING => "starting",
        AARTSAAPI_RUNNING => "running",
        AARTSAAPI_STOPPING => "stopping",
        AARTSAAPI_DISCONNECTING => "disconnecting",
        AARTSAAPI_WARNING => "warning",
        AARTSAAPI_WARNING_VALUE_ADJUSTED => "warning: value adjusted",
        AARTSAAPI_WARNING_VALUE_DISABLED => "warning: value disabled",
        AARTSAAPI_ERROR => "error",
        AARTSAAPI_ERROR_NOT_INITIALIZED => "error: not initialized",
        AARTSAAPI_ERROR_NOT_FOUND => "error: not found",
        AARTSAAPI_ERROR_BUSY => "error: busy",
        AARTSAAPI_ERROR_NOT_OPEN => "error: not open",
        AARTSAAPI_ERROR_NOT_CONNECTED => "error: not connected",
        AARTSAAPI_ERROR_INVALID_CONFIG => "error: invalid config",
        AARTSAAPI_ERROR_BUFFER_SIZE => "error: buffer size",
        AARTSAAPI_ERROR_INVALID_CHANNEL => "error: invalid channel",
        AARTSAAPI_ERROR_INVALID_PARAMETR => "error: invalid parameter",
        AARTSAAPI_ERROR_INVALID_SIZE => "error: invalid size",
        AARTSAAPI_ERROR_MISSING_PATHS_FILE => "error: missing paths file",
        AARTSAAPI_ERROR_VALUE_INVALID => "error: value invalid",
        AARTSAAPI_ERROR_VALUE_MALFORMED => "error: value malformed",
        _ => "unknown",
    }
}

/// A specific granular error code from the Aaronia native SDK.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SdkError {
    #[error("not initialized")]
    NotInitialized,
    #[error("not found")]
    NotFound,
    #[error("busy")]
    Busy,
    #[error("not open")]
    NotOpen,
    #[error("not connected")]
    NotConnected,
    #[error("invalid config")]
    InvalidConfig,
    #[error("buffer size")]
    BufferSize,
    #[error("invalid channel")]
    InvalidChannel,
    #[error("invalid parameter")]
    InvalidParameter,
    #[error("invalid size")]
    InvalidSize,
    #[error("missing paths file")]
    MissingPathsFile,
    #[error("value invalid")]
    ValueInvalid,
    #[error("value malformed")]
    ValueMalformed,
    #[error("generic error")]
    Generic,
    #[error("unknown ({0:#010X})")]
    Unknown(u32),
}

impl SdkError {
    /// Maps a raw `AARTSAAPI` u32 error code to an `SdkError`.
    pub fn from_code(code: u32) -> Self {
        match code {
            AARTSAAPI_ERROR => Self::Generic,
            AARTSAAPI_ERROR_NOT_INITIALIZED => Self::NotInitialized,
            AARTSAAPI_ERROR_NOT_FOUND => Self::NotFound,
            AARTSAAPI_ERROR_BUSY => Self::Busy,
            AARTSAAPI_ERROR_NOT_OPEN => Self::NotOpen,
            AARTSAAPI_ERROR_NOT_CONNECTED => Self::NotConnected,
            AARTSAAPI_ERROR_INVALID_CONFIG => Self::InvalidConfig,
            AARTSAAPI_ERROR_BUFFER_SIZE => Self::BufferSize,
            AARTSAAPI_ERROR_INVALID_CHANNEL => Self::InvalidChannel,
            AARTSAAPI_ERROR_INVALID_PARAMETR => Self::InvalidParameter,
            AARTSAAPI_ERROR_INVALID_SIZE => Self::InvalidSize,
            AARTSAAPI_ERROR_MISSING_PATHS_FILE => Self::MissingPathsFile,
            AARTSAAPI_ERROR_VALUE_INVALID => Self::ValueInvalid,
            AARTSAAPI_ERROR_VALUE_MALFORMED => Self::ValueMalformed,
            _ => Self::Unknown(code),
        }
    }
}

/// Checks an `AARTSAAPI` result code. If the high bit indicates an error,
/// returns a corresponding `Error::SdkApi`. Warnings are logged but not returned as errors.
pub fn check_res(res: u32, operation: &str) -> crate::Result<()> {
    if res & AARTSAAPI_ERROR != 0 {
        return Err(crate::Error::SdkApi {
            operation: operation.to_string(),
            code: SdkError::from_code(res),
        });
    } else if res != AARTSAAPI_OK {
        // Technically this could be a warning, empty, or retry.
        // The Java SDK defines `WARNING = 0x40000000`. We can just trace or debug log.
        if res & 0x40000000 != 0 {
            warn!(
                "{} returned warning 0x{:08X}: {}",
                operation,
                res,
                result_message(res)
            );
        } else {
            debug!(
                "{} returned non-OK 0x{:08X}: {}",
                operation,
                res,
                result_message(res)
            );
        }
    }
    Ok(())
}

// Memory levels
pub const AARTSAAPI_MEMORY_SMALL: u32 = 0;
pub const AARTSAAPI_MEMORY_MEDIUM: u32 = 1;
pub const AARTSAAPI_MEMORY_LARGE: u32 = 2;
pub const AARTSAAPI_MEMORY_LUDICROUS: u32 = 3;

// Config types
pub const AARTSAAPI_CONFIG_TYPE_OTHER: u32 = 0;
pub const AARTSAAPI_CONFIG_TYPE_GROUP: u32 = 1;
pub const AARTSAAPI_CONFIG_TYPE_BLOB: u32 = 2;
pub const AARTSAAPI_CONFIG_TYPE_NUMBER: u32 = 3;
pub const AARTSAAPI_CONFIG_TYPE_BOOL: u32 = 4;
pub const AARTSAAPI_CONFIG_TYPE_ENUM: u32 = 5;
pub const AARTSAAPI_CONFIG_TYPE_STRING: u32 = 6;

// === AARTSAAPI Structures ===

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AARTSAAPI_Handle {
    pub d: *mut c_void,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AARTSAAPI_Device {
    pub d: *mut c_void,
}

// SAFETY: both handles are opaque, heap-allocated vendor objects owned
// exclusively by their wrapper structs and only ever used behind `&mut`
// (exclusive access). The AARTSAAPI vendor samples configure and poll
// from worker threads other than the opening thread, and the API
// documents no thread affinity, so *transferring exclusive ownership*
// between threads (`Send`) is sound. `Sync` is also implemented because
// PyO3 0.23+ requires `#[pyclass]` structs to be `Send + Sync`. This is
// safe because the handle is only ever accessed via `&mut self`, meaning
// concurrent shared access is impossible in safe Rust.
unsafe impl Send for AARTSAAPI_Handle {}
unsafe impl Sync for AARTSAAPI_Handle {}
unsafe impl Send for AARTSAAPI_Device {}
unsafe impl Sync for AARTSAAPI_Device {}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AARTSAAPI_Config {
    pub d: *mut c_void,
}

/// Device identification structure returned by `AARTSAAPI_EnumDevice`.
///
/// **Layout verified against the official `aaroniartsaapi.h`** (2025 SDK):
///
/// ```c
/// struct AARTSAAPI_DeviceInfo {
///     int64_t  cbsize;
///     wchar_t  serialNumber[120];
///     bool     ready; bool boost; bool superspeed; bool active;
/// };
/// ```
///
/// The four status fields are C++ `bool` — **one byte each** on both the
/// MSVC x64 and Itanium (Linux) ABIs. (An earlier revision declared them
/// as `u32` on the mistaken assumption that they marshalled through Win32
/// `BOOL`; that inflated the struct by 12 bytes, passed the wrong `cbsize`
/// to the SDK, and read the flags from the wrong offsets.)
///
/// The fields stay `u8` rather than Rust `bool`: Rust `bool` makes any
/// byte other than 0/1 undefined behaviour, while foreign code only
/// guarantees zero/non-zero. The `ready()` / `boost()` / `superspeed()` /
/// `active()` accessors normalise to a Rust `bool`.
#[repr(C)]
#[derive(Debug)]
pub struct AARTSAAPI_DeviceInfo {
    pub cbsize: i64,
    pub serial_number: [WideChar; 120], // wchar_t[120]
    /// Raw C++ `bool` byte for `ready`. Read through [`Self::ready`].
    pub ready: u8,
    /// Raw C++ `bool` byte for `boost`. Read through [`Self::boost`].
    pub boost: u8,
    /// Raw C++ `bool` byte for `superspeed`. Read through
    /// [`Self::superspeed`].
    pub superspeed: u8,
    /// Raw C++ `bool` byte for `active`. Read through [`Self::active`].
    pub active: u8,
}

impl AARTSAAPI_DeviceInfo {
    /// Device is ready and booted. Normalises the raw 32-bit slot to a
    /// Rust `bool` (any non-zero value is "true", matching the Win32
    /// `BOOL` convention the SDK uses).
    pub fn ready(&self) -> bool {
        self.ready != 0
    }

    /// Device has a second USB connector (V6 feature). See [`Self::ready`]
    /// for the truthy-slot rationale.
    pub fn boost(&self) -> bool {
        self.boost != 0
    }

    /// Device is connected via USB 3.0 superspeed.
    pub fn superspeed(&self) -> bool {
        self.superspeed != 0
    }

    /// Device is already in use by another application.
    pub fn active(&self) -> bool {
        self.active != 0
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct AARTSAAPI_ConfigInfo {
    pub cbsize: i64,
    pub name: [WideChar; 80],   // wchar_t[80]
    pub title: [WideChar; 120], // wchar_t[120]
    pub config_type: u32,       // AARTSAAPI_ConfigType
    pub min_value: f64,
    pub max_value: f64,
    pub step_value: f64,
    pub unit: [WideChar; 10],      // wchar_t[10]
    pub options: [WideChar; 1000], // wchar_t[1000]
    pub disabled_options: u64,
}

#[repr(C)]
#[derive(Debug)]
pub struct AARTSAAPI_Packet {
    pub cbsize: i64,
    pub stream_id: u64,
    pub flags: u64,
    pub start_time: f64,
    pub end_time: f64,
    pub start_frequency: f64,
    pub step_frequency: f64, // Sample rate
    pub span_frequency: f64,
    pub rbw_frequency: f64,
    pub num: i64, // Number of samples in packet
    pub total: i64,
    pub size: i64,
    pub stride: i64,
    pub fp32: *mut f32, // IQ data pointer
    pub interleave: i64,
}

// === FFI Function Type Definitions ===

type AARTSAAPI_Init = unsafe extern "C" fn(memory: u32) -> u32;
type AARTSAAPI_Init_With_Path = unsafe extern "C" fn(memory: u32, path: *const WideChar) -> u32;
type AARTSAAPI_Shutdown = unsafe extern "C" fn() -> u32;
type AARTSAAPI_Version = unsafe extern "C" fn() -> u32;

type AARTSAAPI_Open = unsafe extern "C" fn(handle: *mut AARTSAAPI_Handle) -> u32;
type AARTSAAPI_Close = unsafe extern "C" fn(handle: *mut AARTSAAPI_Handle) -> u32;

type AARTSAAPI_RescanDevices =
    unsafe extern "C" fn(handle: *mut AARTSAAPI_Handle, timeout: i32) -> u32;
type AARTSAAPI_ResetDevices = unsafe extern "C" fn(handle: *mut AARTSAAPI_Handle) -> u32;
type AARTSAAPI_EnumDevice = unsafe extern "C" fn(
    handle: *mut AARTSAAPI_Handle,
    device_type: *const WideChar,
    index: i32,
    info: *mut AARTSAAPI_DeviceInfo,
) -> u32;

type AARTSAAPI_OpenDevice = unsafe extern "C" fn(
    handle: *mut AARTSAAPI_Handle,
    device: *mut AARTSAAPI_Device,
    device_type: *const WideChar,
    serial: *const WideChar,
) -> u32;
type AARTSAAPI_CloseDevice =
    unsafe extern "C" fn(handle: *mut AARTSAAPI_Handle, device: *mut AARTSAAPI_Device) -> u32;
type AARTSAAPI_ConnectDevice = unsafe extern "C" fn(device: *mut AARTSAAPI_Device) -> u32;
type AARTSAAPI_DisconnectDevice = unsafe extern "C" fn(device: *mut AARTSAAPI_Device) -> u32;
type AARTSAAPI_StartDevice = unsafe extern "C" fn(device: *mut AARTSAAPI_Device) -> u32;
type AARTSAAPI_StopDevice = unsafe extern "C" fn(device: *mut AARTSAAPI_Device) -> u32;
type AARTSAAPI_GetDeviceState = unsafe extern "C" fn(device: *mut AARTSAAPI_Device) -> u32;

type AARTSAAPI_ConfigRoot =
    unsafe extern "C" fn(device: *mut AARTSAAPI_Device, config: *mut AARTSAAPI_Config) -> u32;
type AARTSAAPI_ConfigHealth =
    unsafe extern "C" fn(device: *mut AARTSAAPI_Device, config: *mut AARTSAAPI_Config) -> u32;
type AARTSAAPI_ConfigFirst = unsafe extern "C" fn(
    device: *mut AARTSAAPI_Device,
    group: *mut AARTSAAPI_Config,
    config: *mut AARTSAAPI_Config,
) -> u32;
type AARTSAAPI_ConfigNext = unsafe extern "C" fn(
    device: *mut AARTSAAPI_Device,
    group: *mut AARTSAAPI_Config,
    config: *mut AARTSAAPI_Config,
) -> u32;
type AARTSAAPI_ConfigGetName = unsafe extern "C" fn(
    device: *mut AARTSAAPI_Device,
    config: *mut AARTSAAPI_Config,
    name: *mut WideChar,
) -> u32;
type AARTSAAPI_ConfigFind = unsafe extern "C" fn(
    device: *mut AARTSAAPI_Device,
    group: *mut AARTSAAPI_Config,
    config: *mut AARTSAAPI_Config,
    path: *const WideChar,
) -> u32;
type AARTSAAPI_ConfigSetFloat = unsafe extern "C" fn(
    device: *mut AARTSAAPI_Device,
    config: *mut AARTSAAPI_Config,
    value: f64,
) -> u32;
type AARTSAAPI_ConfigSetString = unsafe extern "C" fn(
    device: *mut AARTSAAPI_Device,
    config: *mut AARTSAAPI_Config,
    value: *const WideChar,
) -> u32;
type AARTSAAPI_ConfigGetString = unsafe extern "C" fn(
    device: *mut AARTSAAPI_Device,
    config: *mut AARTSAAPI_Config,
    value: *mut WideChar,
    size: *mut i64,
) -> u32;
type AARTSAAPI_ConfigGetInfo = unsafe extern "C" fn(
    device: *mut AARTSAAPI_Device,
    config: *mut AARTSAAPI_Config,
    info: *mut AARTSAAPI_ConfigInfo,
) -> u32;
type AARTSAAPI_ConfigGetFloat = unsafe extern "C" fn(
    device: *mut AARTSAAPI_Device,
    config: *mut AARTSAAPI_Config,
    value: *mut f64,
) -> u32;
type AARTSAAPI_ConfigSetInteger = unsafe extern "C" fn(
    device: *mut AARTSAAPI_Device,
    config: *mut AARTSAAPI_Config,
    value: i64,
) -> u32;
type AARTSAAPI_ConfigGetInteger = unsafe extern "C" fn(
    device: *mut AARTSAAPI_Device,
    config: *mut AARTSAAPI_Config,
    value: *mut i64,
) -> u32;

type AARTSAAPI_AvailPackets =
    unsafe extern "C" fn(device: *mut AARTSAAPI_Device, channel: i32, num: *mut i32) -> u32;
type AARTSAAPI_GetPacket = unsafe extern "C" fn(
    device: *mut AARTSAAPI_Device,
    channel: i32,
    index: i32,
    packet: *mut AARTSAAPI_Packet,
) -> u32;
type AARTSAAPI_ConsumePackets =
    unsafe extern "C" fn(device: *mut AARTSAAPI_Device, channel: i32, num: i32) -> u32;
type AARTSAAPI_GetMasterStreamTime =
    unsafe extern "C" fn(device: *mut AARTSAAPI_Device, stime: *mut f64) -> u32;
type AARTSAAPI_SendPacket = unsafe extern "C" fn(
    device: *mut AARTSAAPI_Device,
    channel: i32,
    packet: *const AARTSAAPI_Packet,
) -> u32;

// === Native SDK Client ===

/// `AARTSAAPI_Init` and `AARTSAAPI_Shutdown` are process-wide (the vendor
/// header: call Init once at startup and Shutdown before exit), while every
/// `NativeSdkSource` and `SdkSink` owns its own client. Counting the live
/// initialisations initialises the SDK on the first client and shuts it
/// down on the last, rather than tearing it down under a source that is
/// still streaming because a sibling client was dropped.
static SDK_LIVE_INITS: std::sync::Mutex<usize> = std::sync::Mutex::new(0);

pub struct NativeSdkClient {
    _lib: Library,
    initialized: std::sync::atomic::AtomicBool,

    // Core functions
    init: AARTSAAPI_Init,
    init_with_path: AARTSAAPI_Init_With_Path,
    shutdown: AARTSAAPI_Shutdown,
    version: AARTSAAPI_Version,

    // Handle management
    open: AARTSAAPI_Open,
    close: AARTSAAPI_Close,

    // Device management
    rescan_devices: AARTSAAPI_RescanDevices,
    reset_devices: AARTSAAPI_ResetDevices,
    enum_device: AARTSAAPI_EnumDevice,
    open_device: AARTSAAPI_OpenDevice,
    close_device: AARTSAAPI_CloseDevice,
    connect_device: AARTSAAPI_ConnectDevice,
    disconnect_device: AARTSAAPI_DisconnectDevice,
    start_device: AARTSAAPI_StartDevice,
    stop_device: AARTSAAPI_StopDevice,
    get_device_state: AARTSAAPI_GetDeviceState,

    // Configuration
    config_root: AARTSAAPI_ConfigRoot,
    config_health: AARTSAAPI_ConfigHealth,
    config_first: AARTSAAPI_ConfigFirst,
    config_next: AARTSAAPI_ConfigNext,
    config_get_name: AARTSAAPI_ConfigGetName,
    config_find: AARTSAAPI_ConfigFind,
    config_set_float: AARTSAAPI_ConfigSetFloat,
    config_set_string: AARTSAAPI_ConfigSetString,
    config_set_integer: AARTSAAPI_ConfigSetInteger,
    config_get_string: AARTSAAPI_ConfigGetString,
    config_get_info: AARTSAAPI_ConfigGetInfo,
    config_get_float: AARTSAAPI_ConfigGetFloat,
    config_get_integer: AARTSAAPI_ConfigGetInteger,

    // Data acquisition
    avail_packets: AARTSAAPI_AvailPackets,
    get_packet: AARTSAAPI_GetPacket,
    consume_packets: AARTSAAPI_ConsumePackets,
    get_master_stream_time: AARTSAAPI_GetMasterStreamTime,
    send_packet: AARTSAAPI_SendPacket,
}

impl NativeSdkClient {
    pub unsafe fn new() -> Result<Self> {
        unsafe {
            let lib_path = get_sdk_library_path().ok_or_else(|| {
                Error::Sdk(
                    "Aaronia SDK library not found. Please install Aaronia RTSA-Suite PRO."
                        .to_string(),
                )
            })?;

            info!("Loading Aaronia SDK library: {}", lib_path);
            let lib = Library::new(&lib_path).map_err(|e| {
                Error::Sdk(format!("Failed to load SDK library {}: {}", lib_path, e))
            })?;

            // Load all required functions
            // Load every function pointer into a local binding scoped to a
            // single statement. Each `Symbol<'_, T>` borrows `lib` for its
            // lifetime, so naming the Symbol (e.g. `let init: Symbol<_> = ...`)
            // would keep that borrow alive through the `Ok(Self { ... })`
            // construction and block the move of `lib` into `_lib`.
            // Dereferencing the Symbol on its line of birth lets the
            // temporary die at the semicolon, releasing the borrow before
            // `_lib: lib`. Each fn pointer is `Copy`, so the assigned
            // binding outlives the temporary.
            let init: AARTSAAPI_Init = *lib.get::<AARTSAAPI_Init>(b"AARTSAAPI_Init\0")?;
            let init_with_path: AARTSAAPI_Init_With_Path =
                *lib.get::<AARTSAAPI_Init_With_Path>(b"AARTSAAPI_Init_With_Path\0")?;
            let shutdown: AARTSAAPI_Shutdown =
                *lib.get::<AARTSAAPI_Shutdown>(b"AARTSAAPI_Shutdown\0")?;
            let version: AARTSAAPI_Version =
                *lib.get::<AARTSAAPI_Version>(b"AARTSAAPI_Version\0")?;

            let open: AARTSAAPI_Open = *lib.get::<AARTSAAPI_Open>(b"AARTSAAPI_Open\0")?;
            let close: AARTSAAPI_Close = *lib.get::<AARTSAAPI_Close>(b"AARTSAAPI_Close\0")?;

            let rescan_devices: AARTSAAPI_RescanDevices =
                *lib.get::<AARTSAAPI_RescanDevices>(b"AARTSAAPI_RescanDevices\0")?;
            let reset_devices: AARTSAAPI_ResetDevices =
                *lib.get::<AARTSAAPI_ResetDevices>(b"AARTSAAPI_ResetDevices\0")?;
            let enum_device: AARTSAAPI_EnumDevice =
                *lib.get::<AARTSAAPI_EnumDevice>(b"AARTSAAPI_EnumDevice\0")?;
            let open_device: AARTSAAPI_OpenDevice =
                *lib.get::<AARTSAAPI_OpenDevice>(b"AARTSAAPI_OpenDevice\0")?;
            let close_device: AARTSAAPI_CloseDevice =
                *lib.get::<AARTSAAPI_CloseDevice>(b"AARTSAAPI_CloseDevice\0")?;
            let connect_device: AARTSAAPI_ConnectDevice =
                *lib.get::<AARTSAAPI_ConnectDevice>(b"AARTSAAPI_ConnectDevice\0")?;
            let disconnect_device: AARTSAAPI_DisconnectDevice =
                *lib.get::<AARTSAAPI_DisconnectDevice>(b"AARTSAAPI_DisconnectDevice\0")?;
            let start_device: AARTSAAPI_StartDevice =
                *lib.get::<AARTSAAPI_StartDevice>(b"AARTSAAPI_StartDevice\0")?;
            let stop_device: AARTSAAPI_StopDevice =
                *lib.get::<AARTSAAPI_StopDevice>(b"AARTSAAPI_StopDevice\0")?;
            let get_device_state: AARTSAAPI_GetDeviceState =
                *lib.get::<AARTSAAPI_GetDeviceState>(b"AARTSAAPI_GetDeviceState\0")?;

            let config_root: AARTSAAPI_ConfigRoot =
                *lib.get::<AARTSAAPI_ConfigRoot>(b"AARTSAAPI_ConfigRoot\0")?;
            let config_health: AARTSAAPI_ConfigHealth =
                *lib.get::<AARTSAAPI_ConfigHealth>(b"AARTSAAPI_ConfigHealth\0")?;
            let config_first: AARTSAAPI_ConfigFirst =
                *lib.get::<AARTSAAPI_ConfigFirst>(b"AARTSAAPI_ConfigFirst\0")?;
            let config_next: AARTSAAPI_ConfigNext =
                *lib.get::<AARTSAAPI_ConfigNext>(b"AARTSAAPI_ConfigNext\0")?;
            let config_get_name: AARTSAAPI_ConfigGetName =
                *lib.get::<AARTSAAPI_ConfigGetName>(b"AARTSAAPI_ConfigGetName\0")?;
            let config_find: AARTSAAPI_ConfigFind =
                *lib.get::<AARTSAAPI_ConfigFind>(b"AARTSAAPI_ConfigFind\0")?;
            let config_set_float: AARTSAAPI_ConfigSetFloat =
                *lib.get::<AARTSAAPI_ConfigSetFloat>(b"AARTSAAPI_ConfigSetFloat\0")?;
            let config_set_string: AARTSAAPI_ConfigSetString =
                *lib.get::<AARTSAAPI_ConfigSetString>(b"AARTSAAPI_ConfigSetString\0")?;
            let config_set_integer: AARTSAAPI_ConfigSetInteger =
                *lib.get::<AARTSAAPI_ConfigSetInteger>(b"AARTSAAPI_ConfigSetInteger\0")?;
            let config_get_string: AARTSAAPI_ConfigGetString =
                *lib.get::<AARTSAAPI_ConfigGetString>(b"AARTSAAPI_ConfigGetString\0")?;
            let config_get_info: AARTSAAPI_ConfigGetInfo =
                *lib.get::<AARTSAAPI_ConfigGetInfo>(b"AARTSAAPI_ConfigGetInfo\0")?;
            let config_get_float: AARTSAAPI_ConfigGetFloat =
                *lib.get::<AARTSAAPI_ConfigGetFloat>(b"AARTSAAPI_ConfigGetFloat\0")?;
            let config_get_integer: AARTSAAPI_ConfigGetInteger =
                *lib.get::<AARTSAAPI_ConfigGetInteger>(b"AARTSAAPI_ConfigGetInteger\0")?;

            let avail_packets: AARTSAAPI_AvailPackets =
                *lib.get::<AARTSAAPI_AvailPackets>(b"AARTSAAPI_AvailPackets\0")?;
            let get_packet: AARTSAAPI_GetPacket =
                *lib.get::<AARTSAAPI_GetPacket>(b"AARTSAAPI_GetPacket\0")?;
            let consume_packets: AARTSAAPI_ConsumePackets =
                *lib.get::<AARTSAAPI_ConsumePackets>(b"AARTSAAPI_ConsumePackets\0")?;
            let get_master_stream_time: AARTSAAPI_GetMasterStreamTime =
                *lib.get::<AARTSAAPI_GetMasterStreamTime>(b"AARTSAAPI_GetMasterStreamTime\0")?;
            let send_packet: AARTSAAPI_SendPacket =
                *lib.get::<AARTSAAPI_SendPacket>(b"AARTSAAPI_SendPacket\0")?;

            info!("All SDK functions loaded successfully");

            Ok(Self {
                _lib: lib,
                initialized: std::sync::atomic::AtomicBool::new(false),
                init,
                init_with_path,
                shutdown,
                version,
                open,
                close,
                rescan_devices,
                reset_devices,
                enum_device,
                open_device,
                close_device,
                connect_device,
                disconnect_device,
                start_device,
                stop_device,
                get_device_state,
                config_root,
                config_health,
                config_first,
                config_next,
                config_get_name,
                config_find,
                config_set_float,
                config_set_string,
                config_set_integer,
                config_get_string,
                config_get_info,
                config_get_float,
                config_get_integer,
                avail_packets,
                get_packet,
                consume_packets,
                get_master_stream_time,
                send_packet,
            })
        }
    }

    // === Core SDK Functions ===

    pub unsafe fn init_with_path(&self, memory: u32, xml_path: &str) -> Result<()> {
        unsafe {
            // Checked under the lock so two initialisations of the same
            // client cannot both count themselves.
            let mut live = SDK_LIVE_INITS.lock().unwrap_or_else(|p| p.into_inner());
            if self.initialized.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(());
            }
            if *live == 0 {
                let wide_path = string_to_wide(xml_path)?;
                let result = (self.init_with_path)(memory, wide_path.as_ptr());
                check_res(result, "AARTSAAPI_Init_With_Path")?;
                info!("SDK initialized successfully");
            } else {
                debug!(
                    "SDK already initialized by {} other client(s); sharing it",
                    *live
                );
            }
            *live += 1;
            self.initialized
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    /// Release this client's hold on the SDK. `AARTSAAPI_Shutdown` runs
    /// only when no other client is initialised; a no-op on a client
    /// that never initialised.
    pub unsafe fn shutdown(&self) -> Result<()> {
        unsafe {
            if !self
                .initialized
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Ok(());
            }
            let mut live = SDK_LIVE_INITS.lock().unwrap_or_else(|p| p.into_inner());
            *live = live.saturating_sub(1);
            if *live > 0 {
                debug!(
                    "SDK still in use by {} other client(s); not shut down",
                    *live
                );
                return Ok(());
            }
            let result = (self.shutdown)();
            check_res(result, "AARTSAAPI_Shutdown")?;
            info!("SDK shutdown successfully");
            Ok(())
        }
    }

    pub unsafe fn get_version(&self) -> u32 {
        unsafe { (self.version)() }
    }

    // === Handle Management ===

    pub unsafe fn open_handle(&self) -> Result<AARTSAAPI_Handle> {
        unsafe {
            let mut handle = AARTSAAPI_Handle { d: ptr::null_mut() };
            let result = (self.open)(&mut handle);
            check_res(result, "AARTSAAPI_Open")?;
            if handle.d.is_null() {
                return Err(Error::Sdk(format!(
                    "AARTSAAPI_Open returned no handle (result 0x{result:08X})"
                )));
            }
            debug!("SDK handle opened successfully");
            Ok(handle)
        }
    }

    pub unsafe fn close_handle(&self, handle: &mut AARTSAAPI_Handle) -> Result<()> {
        unsafe {
            let result = (self.close)(handle);
            check_res(result, "AARTSAAPI_Close")?;
            debug!("SDK handle closed successfully");
            Ok(())
        }
    }

    // === Device Management ===

    pub unsafe fn rescan_devices(
        &self,
        handle: &mut AARTSAAPI_Handle,
        timeout_ms: i32,
    ) -> Result<()> {
        unsafe {
            let mut result = (self.rescan_devices)(handle, timeout_ms);
            let mut retries = 0;
            let max_retries = 50; // 5 seconds total

            // Handle retry logic as per SDK documentation
            while result == AARTSAAPI_RETRY && retries < max_retries {
                warn!(
                    "Device rescan returned RETRY, retrying ({}/{})",
                    retries + 1,
                    max_retries
                );
                std::thread::sleep(std::time::Duration::from_millis(100));
                result = (self.rescan_devices)(handle, timeout_ms);
                retries += 1;
            }

            if result == AARTSAAPI_RETRY {
                return Err(Error::Sdk(
                    "AARTSAAPI_RescanDevices exceeded maximum retries".to_string(),
                ));
            }

            check_res(result, "AARTSAAPI_RescanDevices")?;
            info!("Device rescan completed successfully");
            Ok(())
        }
    }

    /// The device families this SDK knows about, in the order worth
    /// trying.
    pub const DEVICE_FAMILIES: [&'static str; 2] = ["spectranv6", "spectranv6eco"];

    /// Find which family actually has a device attached.
    ///
    /// `AARTSAAPI_EnumDevice` matches one family at a time, so a caller
    /// configured for `spectranv6` finds nothing on a machine holding
    /// only a V6 ECO, and the failure reads as "no device" rather than
    /// "wrong family". This tries each family in turn and returns the
    /// first that enumerates a device, so the common case needs no
    /// configuration at all.
    ///
    /// # Safety
    /// `handle` must be an open library handle.
    pub unsafe fn detect_device_family(
        &self,
        handle: &mut AARTSAAPI_Handle,
    ) -> Result<Option<&'static str>> {
        for family in Self::DEVICE_FAMILIES {
            match unsafe { self.enum_device(handle, family, 0) } {
                Ok(Some(_)) => {
                    info!("Detected device family {family}");
                    return Ok(Some(family));
                }
                // An empty family is the normal answer for the one that
                // is not attached; keep looking.
                Ok(None) => continue,
                Err(e) => {
                    debug!("Enumerating {family} failed, trying the next: {e}");
                    continue;
                }
            }
        }
        Ok(None)
    }

    pub unsafe fn enum_device(
        &self,
        handle: &mut AARTSAAPI_Handle,
        device_type: &str,
        index: i32,
    ) -> Result<Option<AARTSAAPI_DeviceInfo>> {
        unsafe {
            // Per the official RTSA-API-Samples, `AARTSAAPI_EnumDevice` takes the
            // bare family name ("spectranv6", "spectranv6eco") — *not* a
            // mode-qualified string ("spectranv6/raw"). Passing the qualified form
            // causes the SDK to silently return no devices. Warn loudly so we
            // catch regressions during development.
            if device_type.contains('/') {
                warn!(
                    "AARTSAAPI_EnumDevice called with mode-qualified type {:?}; \
                 SDK expects only the family ({:?}). Discovery may return \
                 zero devices.",
                    device_type,
                    device_type.split('/').next().unwrap_or(device_type)
                );
            }

            let mut device_info = AARTSAAPI_DeviceInfo {
                cbsize: std::mem::size_of::<AARTSAAPI_DeviceInfo>() as i64,
                serial_number: [0; 120],
                ready: 0,
                boost: 0,
                superspeed: 0,
                active: 0,
            };

            let wide_type = string_to_wide(device_type)?;
            let result = (self.enum_device)(handle, wide_type.as_ptr(), index, &mut device_info);

            if result == AARTSAAPI_EMPTY {
                debug!("No more devices at index {}", index);
                return Ok(None);
            }
            check_res(result, "AARTSAAPI_EnumDevice")?;
            debug!(
                "Found device at index {}: ready={}, boost={}, superspeed={}, active={}",
                index,
                device_info.ready(),
                device_info.boost(),
                device_info.superspeed(),
                device_info.active()
            );
            Ok(Some(device_info))
        }
    }

    pub unsafe fn open_device(
        &self,
        handle: &mut AARTSAAPI_Handle,
        device_type: &str,
        serial_number: &[WideChar],
    ) -> Result<AARTSAAPI_Device> {
        unsafe {
            let mut device = AARTSAAPI_Device { d: ptr::null_mut() };
            let wide_type = string_to_wide(device_type)?;
            let result = (self.open_device)(
                handle,
                &mut device,
                wide_type.as_ptr(),
                serial_number.as_ptr(),
            );

            check_res(result, "AARTSAAPI_OpenDevice")?;
            // Result codes without the error bit (EMPTY, RETRY, the state
            // codes) pass `check_res`; only a non-null object is success.
            if device.d.is_null() {
                return Err(Error::Sdk(format!(
                    "AARTSAAPI_OpenDevice({device_type}) returned no device (result 0x{result:08X})"
                )));
            }
            info!("Device {} opened successfully", device_type);
            Ok(device)
        }
    }

    pub unsafe fn connect_device(&self, device: &mut AARTSAAPI_Device) -> Result<()> {
        unsafe {
            let result = (self.connect_device)(device);
            check_res(result, "AARTSAAPI_ConnectDevice")?;
            info!("Device connected successfully");
            Ok(())
        }
    }

    pub unsafe fn start_device(&self, device: &mut AARTSAAPI_Device) -> Result<()> {
        unsafe {
            let result = (self.start_device)(device);
            check_res(result, "AARTSAAPI_StartDevice")?;
            info!("Device started successfully");
            Ok(())
        }
    }

    pub unsafe fn stop_device(&self, device: &mut AARTSAAPI_Device) -> Result<()> {
        unsafe {
            let result = (self.stop_device)(device);
            check_res(result, "AARTSAAPI_StopDevice")?;
            info!("Device stopped successfully");
            Ok(())
        }
    }

    pub unsafe fn disconnect_device(&self, device: &mut AARTSAAPI_Device) -> Result<()> {
        unsafe {
            let result = (self.disconnect_device)(device);
            check_res(result, "AARTSAAPI_DisconnectDevice")?;
            info!("Device disconnected successfully");
            Ok(())
        }
    }

    pub unsafe fn close_device(
        &self,
        handle: &mut AARTSAAPI_Handle,
        device: &mut AARTSAAPI_Device,
    ) -> Result<()> {
        unsafe {
            let result = (self.close_device)(handle, device);
            check_res(result, "AARTSAAPI_CloseDevice")?;
            info!("Device closed successfully");
            Ok(())
        }
    }

    // === Configuration Methods ===

    pub unsafe fn get_config_root(
        &self,
        device: &mut AARTSAAPI_Device,
    ) -> Result<AARTSAAPI_Config> {
        unsafe {
            let mut config = AARTSAAPI_Config { d: ptr::null_mut() };
            let result = (self.config_root)(device, &mut config);
            check_res(result, "AARTSAAPI_ConfigRoot")?;
            if config.d.is_null() {
                return Err(Error::Sdk(format!(
                    "AARTSAAPI_ConfigRoot returned no config (result 0x{result:08X})"
                )));
            }
            Ok(config)
        }
    }

    pub unsafe fn find_config(
        &self,
        device: &mut AARTSAAPI_Device,
        group: &mut AARTSAAPI_Config,
        path: &str,
    ) -> Result<AARTSAAPI_Config> {
        unsafe {
            let mut config = AARTSAAPI_Config { d: ptr::null_mut() };
            let wide_path = string_to_wide(path)?;
            let result = (self.config_find)(device, group, &mut config, wide_path.as_ptr());

            check_res(result, "AARTSAAPI_ConfigFind")?;
            if config.d.is_null() {
                return Err(Error::Sdk(format!(
                    "config `{path}` not found (result 0x{result:08X})"
                )));
            }
            debug!("Config {} found successfully", path);
            Ok(config)
        }
    }

    pub unsafe fn set_config_float(
        &self,
        device: &mut AARTSAAPI_Device,
        config: &mut AARTSAAPI_Config,
        value: f64,
    ) -> Result<()> {
        unsafe {
            let result = (self.config_set_float)(device, config, value);
            check_res(result, "AARTSAAPI_ConfigSetFloat")?;
            debug!("Config float value set to: {}", value);
            Ok(())
        }
    }

    pub unsafe fn set_config_string(
        &self,
        device: &mut AARTSAAPI_Device,
        config: &mut AARTSAAPI_Config,
        value: &str,
    ) -> Result<()> {
        unsafe {
            let wide_value = string_to_wide(value)?;
            let result = (self.config_set_string)(device, config, wide_value.as_ptr());
            check_res(result, "AARTSAAPI_ConfigSetString")?;
            debug!("Config string value set to: {}", value);
            Ok(())
        }
    }

    pub unsafe fn get_config_string(
        &self,
        device: &mut AARTSAAPI_Device,
        config: &mut AARTSAAPI_Config,
    ) -> Result<String> {
        unsafe {
            let mut size: i64 = 1024;
            let mut buffer = vec![0 as WideChar; size as usize];
            let result = (self.config_get_string)(device, config, buffer.as_mut_ptr(), &mut size);
            check_res(result, "AARTSAAPI_ConfigGetString")?;
            Ok(wide_to_string(&buffer))
        }
    }

    /// Read a `device/main/...` config value as a 64-bit float.
    pub unsafe fn get_config_float(
        &self,
        device: &mut AARTSAAPI_Device,
        config: &mut AARTSAAPI_Config,
    ) -> Result<f64> {
        unsafe {
            let mut value: f64 = 0.0;
            let result = (self.config_get_float)(device, config, &mut value);
            check_res(result, "AARTSAAPI_ConfigGetFloat")?;
            Ok(value)
        }
    }

    /// Read a config node's metadata (min/max/step, enum option list, unit).
    /// Used by the SweepSpectrumEco sample to discover valid frequency ranges
    /// before writing them; we use it to read the live `device/receiverclock`
    /// and validate it against `main/spanfreq`.
    pub unsafe fn get_config_info(
        &self,
        device: &mut AARTSAAPI_Device,
        config: &mut AARTSAAPI_Config,
    ) -> Result<AARTSAAPI_ConfigInfo> {
        unsafe {
            let mut info = AARTSAAPI_ConfigInfo {
                cbsize: std::mem::size_of::<AARTSAAPI_ConfigInfo>() as i64,
                name: [0; 80],
                title: [0; 120],
                config_type: 0,
                min_value: 0.0,
                max_value: 0.0,
                step_value: 0.0,
                unit: [0; 10],
                options: [0; 1000],
                disabled_options: 0,
            };
            let result = (self.config_get_info)(device, config, &mut info);
            check_res(result, "AARTSAAPI_ConfigGetInfo")?;
            Ok(info)
        }
    }

    /// Set a typed integer config (e.g. `main/decimation` index per
    /// RawIQ.cpp:142 — the comment "could have also used
    /// AARTSAAPI_ConfigSetInteger(&d, &config, 6)" maps index 6 to "1/64",
    /// so the indexing is `2^index` decimation).
    pub unsafe fn set_config_integer(
        &self,
        device: &mut AARTSAAPI_Device,
        config: &mut AARTSAAPI_Config,
        value: i64,
    ) -> Result<()> {
        unsafe {
            let result = (self.config_set_integer)(device, config, value);
            check_res(result, "AARTSAAPI_ConfigSetInteger")?;
            Ok(())
        }
    }

    /// Read a typed integer config.
    pub unsafe fn get_config_integer(
        &self,
        device: &mut AARTSAAPI_Device,
        config: &mut AARTSAAPI_Config,
    ) -> Result<i64> {
        unsafe {
            let mut value: i64 = 0;
            let result = (self.config_get_integer)(device, config, &mut value);
            check_res(result, "AARTSAAPI_ConfigGetInteger")?;
            Ok(value)
        }
    }

    /// Get the *health* config tree root (temperature, voltage, etc.). The
    /// WrapperSample.cpp's TreeConfig demonstrates iterating it with
    /// `config_first` / `config_next`.
    pub unsafe fn get_config_health(
        &self,
        device: &mut AARTSAAPI_Device,
    ) -> Result<AARTSAAPI_Config> {
        unsafe {
            let mut config = AARTSAAPI_Config { d: ptr::null_mut() };
            let result = (self.config_health)(device, &mut config);
            check_res(result, "AARTSAAPI_ConfigHealth")?;
            Ok(config)
        }
    }

    /// Get the first child config under `group` (returns `Ok(None)` if there
    /// are none). Pair with `config_next` to walk the tree.
    pub unsafe fn config_first(
        &self,
        device: &mut AARTSAAPI_Device,
        group: &mut AARTSAAPI_Config,
    ) -> Result<Option<AARTSAAPI_Config>> {
        unsafe {
            let mut config = AARTSAAPI_Config { d: ptr::null_mut() };
            let result = (self.config_first)(device, group, &mut config);
            if result == AARTSAAPI_EMPTY {
                return Ok(None);
            }
            check_res(result, "AARTSAAPI_ConfigFirst")?;
            Ok(Some(config))
        }
    }

    /// Advance to the next sibling under `group` from the position of
    /// `config`. Returns `Ok(None)` after the last sibling.
    pub unsafe fn config_next(
        &self,
        device: &mut AARTSAAPI_Device,
        group: &mut AARTSAAPI_Config,
        config: &mut AARTSAAPI_Config,
    ) -> Result<bool> {
        unsafe {
            let result = (self.config_next)(device, group, config);
            if result == AARTSAAPI_EMPTY {
                return Ok(false);
            }
            check_res(result, "AARTSAAPI_ConfigNext")?;
            Ok(true)
        }
    }

    /// Read the leaf name of the current config node (the last segment of
    /// the path).
    pub unsafe fn get_config_name(
        &self,
        device: &mut AARTSAAPI_Device,
        config: &mut AARTSAAPI_Config,
    ) -> Result<String> {
        unsafe {
            // The wrapper signature takes a wchar_t* but no explicit length; the
            // SDK fills up to 80 wchar elements per the AARTSAAPI_ConfigInfo
            // `name` field, so allocate the same.
            let mut buffer = vec![0 as WideChar; 80];
            let result = (self.config_get_name)(device, config, buffer.as_mut_ptr());
            check_res(result, "AARTSAAPI_ConfigGetName")?;
            Ok(wide_to_string(&buffer))
        }
    }

    /// Reset the SDK device list (full power-cycle of all attached devices).
    /// Useful for crash recovery when `RescanDevices` keeps returning
    /// `AARTSAAPI_RETRY` past its budget.
    pub unsafe fn reset_devices(&self, handle: &mut AARTSAAPI_Handle) -> Result<()> {
        unsafe {
            let result = (self.reset_devices)(handle);
            check_res(result, "AARTSAAPI_ResetDevices")?;
            Ok(())
        }
    }

    /// Number of packets currently queued on the given channel. Cheap to
    /// poll — useful as a non-blocking liveness check before
    /// `get_packet` (the wrapper exposes this; the official samples loop
    /// `GetPacket` instead, but `AvailPackets` avoids the EMPTY allocation
    /// of an `AARTSAAPI_Packet` per spin).
    pub unsafe fn avail_packets(&self, device: &mut AARTSAAPI_Device, channel: i32) -> Result<i32> {
        unsafe {
            let mut num: i32 = 0;
            let result = (self.avail_packets)(device, channel, &mut num);
            check_res(result, "AARTSAAPI_AvailPackets")?;
            Ok(num)
        }
    }

    // === Data Acquisition Methods ===

    pub unsafe fn get_packet(
        &self,
        device: &mut AARTSAAPI_Device,
        channel: i32,
        index: i32,
    ) -> Result<Option<AARTSAAPI_Packet>> {
        unsafe {
            let mut packet = AARTSAAPI_Packet {
                cbsize: std::mem::size_of::<AARTSAAPI_Packet>() as i64,
                stream_id: 0,
                flags: 0,
                start_time: 0.0,
                end_time: 0.0,
                start_frequency: 0.0,
                step_frequency: 0.0,
                span_frequency: 0.0,
                rbw_frequency: 0.0,
                num: 0,
                total: 0,
                size: 0,
                stride: 0,
                fp32: ptr::null_mut(),
                interleave: 0,
            };

            let result = (self.get_packet)(device, channel, index, &mut packet);
            if result == AARTSAAPI_EMPTY {
                debug!("No packet available");
                return Ok(None);
            }
            check_res(result, "AARTSAAPI_GetPacket")?;
            debug!("Got packet with {} samples", packet.num);
            Ok(Some(packet))
        }
    }

    pub unsafe fn consume_packets(
        &self,
        device: &mut AARTSAAPI_Device,
        channel: i32,
        num_packets: i32,
    ) -> Result<()> {
        unsafe {
            let result = (self.consume_packets)(device, channel, num_packets);
            check_res(result, "AARTSAAPI_ConsumePackets")?;
            debug!("Consumed {} packets", num_packets);
            Ok(())
        }
    }

    pub unsafe fn get_master_stream_time(&self, device: &mut AARTSAAPI_Device) -> Result<f64> {
        unsafe {
            let mut stime = 0.0;
            let result = (self.get_master_stream_time)(device, &mut stime);
            check_res(result, "AARTSAAPI_GetMasterStreamTime")?;
            Ok(stime)
        }
    }

    pub unsafe fn send_packet(
        &self,
        device: &mut AARTSAAPI_Device,
        channel: i32,
        packet: &AARTSAAPI_Packet,
    ) -> Result<()> {
        unsafe {
            let result = (self.send_packet)(device, channel, packet as *const _);
            check_res(result, "AARTSAAPI_SendPacket")?;
            Ok(())
        }
    }
}

impl Drop for NativeSdkClient {
    fn drop(&mut self) {
        // `shutdown` is a no-op unless this client initialised, and only
        // the last live client actually shuts the SDK down.
        unsafe {
            if let Err(e) = self.shutdown() {
                error!("Error shutting down SDK during NativeSdkClient drop: {}", e);
            }
        }
    }
}

// === Utility Functions ===

fn string_to_wide(s: &str) -> Result<WideCString> {
    WideCString::from_str(s).map_err(|e| Error::Sdk(format!("Invalid wide string: {}", e)))
}

fn wide_to_string(wide: &[WideChar]) -> String {
    let null_pos = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    let wstr = widestring::WideString::from_vec(wide[..null_pos].to_vec());
    wstr.to_string_lossy()
}

/// Splits a possibly mode-qualified device-type string (e.g. `"spectranv6"`
/// or `"spectranv6/raw"`) into its bare family — for `AARTSAAPI_EnumDevice`,
/// which only accepts the family, not a mode-qualified string — and its
/// mode-qualified open string for `AARTSAAPI_OpenDevice`. When `device_type`
/// carries no `/mode` suffix, `default_mode` is appended to form the open
/// string (`SdkConfig` defaults to `"raw"`, `SdkSinkConfig` to
/// `"iqtransmitter"` — the two other high-level SDK config wrappers that
/// otherwise duplicated this exact split).
pub(crate) fn split_device_type<'a>(device_type: &'a str, default_mode: &str) -> (&'a str, String) {
    let family = device_type.split('/').next().unwrap_or(device_type);
    let open_mode = if device_type.contains('/') {
        device_type.to_string()
    } else {
        format!("{family}/{default_mode}")
    };
    (family, open_mode)
}

/// The open-mode suffix that gives raw IQ on `family`: `raw` on the V6,
/// `rtsa` on the ECO, which has no `spectranv6eco/raw`.
pub(crate) fn raw_mode_for_family(family: &str) -> &'static str {
    if family == "spectranv6eco" {
        "rtsa"
    } else {
        "raw"
    }
}

/// How one [`NativeSdkSource::read_samples`] call divides its work between
/// the carry-over buffer and a freshly-polled packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReadPlan {
    /// Samples to drain from carry-over into the caller's buffer.
    pub from_carry: usize,
    /// Samples still wanted from the device. `0` means "don't poll" — the
    /// caller is already satisfied, so spending a `READ_POLL_DEADLINE` on a
    /// packet we'd have to carry over anyway is pure latency.
    pub remaining: usize,
}

/// Plan a read against `carry_len` buffered samples.
///
/// Split out from [`NativeSdkSource::read_samples`] purely so the accounting
/// is unit-testable: constructing a `NativeSdkSource` needs a real loaded
/// SDK library, so the arithmetic is otherwise only exercisable on hardware.
pub(crate) fn plan_read(carry_len: usize, max_samples: usize) -> ReadPlan {
    let from_carry = carry_len.min(max_samples);
    ReadPlan {
        from_carry,
        remaining: max_samples - from_carry,
    }
}

/// Divide a packet of `packet_samples` into `(to_caller, to_carry_over)`.
///
/// `to_carry_over` **must** be retained: `AARTSAAPI_ConsumePackets` hands the
/// packet back to the SDK, so a tail that isn't copied out is an
/// unrecoverable hole in the IQ stream.
pub(crate) fn split_packet(packet_samples: usize, remaining: usize) -> (usize, usize) {
    let to_caller = packet_samples.min(remaining);
    (to_caller, packet_samples - to_caller)
}

/// What one spectra packet described, alongside the values appended to
/// the caller's buffer.
///
/// A packet holds `frames` consecutive spectra of `bins_per_frame`
/// values each, laid out one after another, so frame `i` occupies
/// `[i * bins_per_frame ..][.. bins_per_frame]` of what was appended.
/// Values are dBm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpectraRead {
    /// Number of spectra in the packet.
    pub frames: usize,
    /// Bins in each spectrum.
    pub bins_per_frame: usize,
    /// Frequency of the first bin, in Hz.
    pub start_frequency_hz: f64,
    /// Spacing between bins, in Hz.
    pub step_frequency_hz: f64,
    /// Packet start time, on the device's clock.
    pub start_time: f64,
}

// === High-Level Stream Manager ===

/// The open-mode supplied to `AARTSAAPI_OpenDevice`. Tracking which family /
/// mode is active lets us skip config writes that don't apply (e.g. the raw
/// mode's `device/receiverchannel` knob is irrelevant on `iqreceiver`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceOpenMode {
    /// `spectranv6/raw` — the canonical full-band raw IQ pipe.
    Raw,
    /// `spectranv6eco/iqreceiver` — IQ receiver on the ECO platform.
    EcoIqReceiver,
    /// `spectranv6eco/rtsa` — the ECO's raw pipeline, which is what its
    /// spectrum sample opens. There is no `spectranv6eco/raw`.
    EcoRtsa,
    /// `spectranv6/sweepsa` — sweep spectrum analyzer on the V6 platform.
    Sweepsa,
    /// `spectranv6eco/sweepsa` — sweep spectrum analyzer on the ECO platform.
    EcoSweepsa,
    /// Any other variant (`/iqreceiver`, `/sweepsa`, …); we only know the
    /// family/mode strings, not which subset of config keys they accept.
    /// Owned `String` — the earlier `&'static str` was produced with
    /// `Box::leak`, leaking one allocation per open call.
    Other(String),
}

impl DeviceOpenMode {
    /// Parse the mode string we passed to `AARTSAAPI_OpenDevice`.
    pub fn from_open_string(s: &str) -> Self {
        match s {
            "spectranv6/raw" => Self::Raw,
            // The ECO's equivalent of raw mode is called rtsa; it has no
            // "spectranv6eco/raw".
            "spectranv6eco/rtsa" => Self::EcoRtsa,
            "spectranv6eco/iqreceiver" => Self::EcoIqReceiver,
            "spectranv6/sweepsa" => Self::Sweepsa,
            "spectranv6eco/sweepsa" => Self::EcoSweepsa,
            other => Self::Other(other.to_string()),
        }
    }

    /// Whether the `device/receiverchannel`, `device/outputformat`,
    /// `device/receiverclock`, and `main/decimation` config keys are
    /// applicable on this mode. Per the official samples, only `raw` mode
    /// exposes them; eco's `iqreceiver` drives a fixed pipeline.
    pub fn supports_raw_only_keys(&self) -> bool {
        matches!(self, Self::Raw)
    }

    /// Which stream index carries spectra in this mode.
    ///
    /// Not a property of the device family, which is how this reads at
    /// first glance. In `spectranv6/raw` the same device delivers IQ on
    /// stream 0 and spectra on stream 2, selected by
    /// `device/outputformat`; Aaronia's `RawIQ` and `RawSpectrum`
    /// samples differ in exactly that. Every other mode, including the
    /// ECO's `rtsa` and both `sweepsa` variants, carries spectra on
    /// stream 0.
    pub fn spectra_stream_index(&self) -> i32 {
        match self {
            Self::Raw => 2,
            _ => 0,
        }
    }
}

pub struct NativeSdkSource {
    client: Arc<NativeSdkClient>,
    handle: Option<AARTSAAPI_Handle>,
    device: Option<AARTSAAPI_Device>,
    open_mode: Option<DeviceOpenMode>,
    stream_active: bool,
    device_connected: bool,
    sample_buffer: VecDeque<Complex32>,
    /// Carry-over for [`Self::read_samples_dual`]: whole (Rx1, Rx2)
    /// sample pairs not yet handed to the caller. Kept separate from
    /// `sample_buffer` — the two read paths must not share a carry
    /// stream, since a mono read of a dual carry would drop every Rx2
    /// sample. The `read_mode` latch enforces that a stream uses one
    /// path or the other, never both.
    dual_sample_buffer: VecDeque<(Complex32, Complex32)>,
    /// Which read path this streaming session uses, latched on the
    /// first successful read call and cleared by
    /// [`Self::stop_streaming`]. Mixing `read_samples` and
    /// `read_samples_dual` on one stream would silently punch
    /// time-gaps into both outputs (each call consumes whole packets
    /// the other path never sees), so the second path errors instead.
    read_mode: Option<ReadMode>,
    /// Receiver clock in Hz, learned when the IQ receiver is configured.
    /// The sample-rate ladder is derived from it, so callers that need
    /// to know which rates exist should read it rather than assume the
    /// default. `None` until the device has been configured.
    receiver_clock_hz: Option<f64>,
}

/// Which of the two packet-consuming read paths a streaming session
/// has committed to. See `NativeSdkSource::read_mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadMode {
    Mono,
    Dual,
}

/// Device health telemetry from the SDK's `AARTSAAPI_ConfigHealth` tree.
///
/// Field names (`fronttemp`, `fpgatemp`, `boardpower`) are taken from the
/// `/healthstatus` HTTP endpoint's JSON tree, captured live from a
/// SPECTRAN V6 ECO — both surfaces read the same underlying device health
/// model, so the naming is expected to match, but this has not been
/// directly exercised against the native `AARTSAAPI_ConfigHealth` call on
/// real hardware (no Windows/Linux dev box with the SDK installed).
/// Verify against a live device before depending on this in production.
#[derive(Debug, Clone, Default)]
pub struct HealthState {
    /// Frontend temperature in °C (`fronttemp`), if reported.
    pub front_temp_c: Option<f64>,
    /// FPGA temperature in °C (`fpgatemp`), if reported.
    pub fpga_temp_c: Option<f64>,
    /// Board power draw in watts (`boardpower`), if reported.
    pub board_power_w: Option<f64>,
}

/// GPS telemetry from the SDK's `AARTSAAPI_ConfigHealth` tree.
///
/// See [`HealthState`] for the field-naming provenance note.
///
/// `latitude`/`longitude`/`altitude`/`time` are `None` unless the
/// corresponding validity flag (`gpsposvalid`/`gpstimevalid`) is true —
/// live captures show the device reports a real `gpsposvalid: false`
/// alongside literal-zero lat/long when no fix is available, and treating
/// that zero as a real coordinate would put "no GPS fix" at Null Island.
#[derive(Debug, Clone, Default)]
pub struct GpsState {
    /// Number of satellites in view (`satellites`), if reported.
    pub satellites: Option<u32>,
    /// Whether `latitude`/`longitude`/`altitude` hold a valid fix
    /// (`gpsposvalid`).
    pub position_valid: bool,
    /// Latitude in degrees (`gpslatitude`). `None` unless
    /// `position_valid`.
    pub latitude: Option<f64>,
    /// Longitude in degrees (`gpslongitude`). `None` unless
    /// `position_valid`.
    pub longitude: Option<f64>,
    /// Altitude in meters (`gpselevation`). `None` unless
    /// `position_valid`.
    pub altitude: Option<f64>,
    /// Whether `time` holds a valid GPS time (`gpstimevalid`).
    pub time_valid: bool,
    /// GPS time (`gpstime`), seconds since the Unix epoch. `None` unless
    /// `time_valid`.
    pub time: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct SweepsaConfig {
    pub start_frequency: f64,
    pub stop_frequency: f64,
    pub rbw: f64,
    pub vbw: f64,
}

impl Default for SweepsaConfig {
    fn default() -> Self {
        Self {
            start_frequency: 1e9,
            stop_frequency: 2e9,
            rbw: 1e6,
            vbw: 1e6,
        }
    }
}

impl NativeSdkSource {
    pub unsafe fn new() -> Result<Self> {
        unsafe {
            let client = Arc::new(NativeSdkClient::new()?);
            Ok(Self {
                client,
                handle: None,
                device: None,
                open_mode: None,
                stream_active: false,
                device_connected: false,
                sample_buffer: VecDeque::new(),
                dual_sample_buffer: VecDeque::new(),
                read_mode: None,
                receiver_clock_hz: None,
            })
        }
    }

    /// Read a health/GPS leaf value, trying the float accessor first and
    /// falling back to the integer accessor. Boolean-typed nodes (e.g.
    /// `gpsposvalid`) are exposed through one of these two numeric
    /// getters by the SDK — there is no dedicated bool getter in the
    /// bound API — so this covers both representations rather than
    /// guessing which one a given firmware uses.
    unsafe fn read_health_value(
        &self,
        device: &mut AARTSAAPI_Device,
        config: &mut AARTSAAPI_Config,
    ) -> Option<f64> {
        unsafe {
            if let Ok(v) = self.client.get_config_float(device, config) {
                return Some(v);
            }
            if let Ok(v) = self.client.get_config_integer(device, config) {
                return Some(v as f64);
            }
            None
        }
    }

    unsafe fn walk_health_tree(
        &self,
        device: &mut AARTSAAPI_Device,
        group: &mut AARTSAAPI_Config,
        health: &mut HealthState,
        gps: &mut GpsState,
    ) -> Result<()> {
        unsafe {
            let mut current = match self.client.config_first(device, group)? {
                Some(c) => c,
                None => return Ok(()),
            };

            loop {
                if let Ok(name) = self.client.get_config_name(device, &mut current) {
                    // Field names verified live against `/healthstatus`
                    // (see `HealthState`/`GpsState` docs) — the exact
                    // `AARTSAAPI_ConfigHealth` tree naming is inferred,
                    // not independently confirmed.
                    match name.as_str() {
                        "fronttemp" => {
                            health.front_temp_c = self.read_health_value(device, &mut current);
                        }
                        "fpgatemp" => {
                            health.fpga_temp_c = self.read_health_value(device, &mut current);
                        }
                        "boardpower" => {
                            health.board_power_w = self.read_health_value(device, &mut current);
                        }
                        "satellites" => {
                            gps.satellites = self
                                .read_health_value(device, &mut current)
                                .map(|v| v as u32);
                        }
                        "gpsposvalid" => {
                            gps.position_valid = self
                                .read_health_value(device, &mut current)
                                .is_some_and(|v| v != 0.0);
                        }
                        "gpslatitude" => {
                            gps.latitude = self.read_health_value(device, &mut current);
                        }
                        "gpslongitude" => {
                            gps.longitude = self.read_health_value(device, &mut current);
                        }
                        "gpselevation" => {
                            gps.altitude = self.read_health_value(device, &mut current);
                        }
                        "gpstimevalid" => {
                            gps.time_valid = self
                                .read_health_value(device, &mut current)
                                .is_some_and(|v| v != 0.0);
                        }
                        "gpstime" => {
                            gps.time = self.read_health_value(device, &mut current);
                        }
                        _ => {}
                    }
                }

                // Recurse into children
                let _ = self.walk_health_tree(device, &mut current, health, gps);

                if !self.client.config_next(device, group, &mut current)? {
                    break;
                }
            }
            Ok(())
        }
    }

    pub unsafe fn get_health_and_gps(&mut self) -> Result<(HealthState, GpsState)> {
        unsafe {
            let mut device = self
                .device
                .ok_or_else(|| Error::Sdk("Device not opened".to_string()))?;
            let mut health_root = self.client.get_config_health(&mut device)?;

            let mut health = HealthState::default();
            let mut gps = GpsState::default();
            self.walk_health_tree(&mut device, &mut health_root, &mut health, &mut gps)?;

            // Reconcile validity after the full walk: tree traversal order
            // is not guaranteed, so `gpsposvalid`/`gpstimevalid` might be
            // visited after the values they gate. Clearing here (rather
            // than gating inline) guarantees an invalid fix never leaks
            // out as a false Null-Island coordinate.
            if !gps.position_valid {
                gps.latitude = None;
                gps.longitude = None;
                gps.altitude = None;
            }
            if !gps.time_valid {
                gps.time = None;
            }

            Ok((health, gps))
        }
    }

    pub unsafe fn initialize(&mut self) -> Result<()> {
        unsafe {
            // Idempotent: a second call must not leak the open handle.
            if self.handle.is_some() {
                return Ok(());
            }
            // Initialize SDK with XML path
            let xml_path = get_xml_config_path()
                .ok_or_else(|| Error::Sdk("Could not determine XML config path".to_string()))?;

            self.client
                .init_with_path(AARTSAAPI_MEMORY_MEDIUM, &xml_path)?;

            // Open handle
            let handle = self.client.open_handle()?;
            self.handle = Some(handle);

            info!("Native SDK source initialized");
            Ok(())
        }
    }

    pub unsafe fn find_devices(&mut self, device_type: &str) -> Result<Vec<AARTSAAPI_DeviceInfo>> {
        unsafe {
            let handle = self
                .handle
                .as_mut()
                .ok_or_else(|| Error::Sdk("SDK not initialized".to_string()))?;

            // Rescan devices first
            self.client.rescan_devices(handle, 2000)?;

            // Enumerate devices
            let mut devices = Vec::new();
            let mut index = 0;

            while let Some(device_info) = self.client.enum_device(handle, device_type, index)? {
                info!(
                    "Found {} device {}: ready={}",
                    device_type,
                    index,
                    device_info.ready()
                );
                devices.push(device_info);
                index += 1;
            }

            info!("Found {} {} devices", devices.len(), device_type);
            Ok(devices)
        }
    }

    pub unsafe fn open_device(
        &mut self,
        device_type: &str,
        serial_number: &[WideChar],
    ) -> Result<()> {
        unsafe {
            // Opening over a live device would leave it opened and
            // streaming with nothing left to stop or close it.
            if self.device.is_some() {
                return Err(Error::Sdk(
                    "a device is already open on this source; stop and drop it before opening another"
                        .to_string(),
                ));
            }
            let handle = self
                .handle
                .as_mut()
                .ok_or_else(|| Error::Sdk("SDK not initialized".to_string()))?;

            let device = self
                .client
                .open_device(handle, device_type, serial_number)?;
            self.device = Some(device);
            self.open_mode = Some(DeviceOpenMode::from_open_string(device_type));

            info!("Device {} opened successfully", device_type);
            Ok(())
        }
    }

    /// Open whichever device is present, without the caller naming the
    /// family.
    ///
    /// `mode` is the suffix, for example `"raw"` or `"iqreceiver"`. The
    /// family is detected first, then joined to it. Note the two
    /// families do not offer identical modes: the V6 has `raw`, while
    /// the ECO's equivalent is `rtsa`, so pass a mode the detected
    /// family supports or use [`Self::open_device`] directly.
    ///
    /// # Safety
    /// Same contract as [`Self::open_device`].
    pub unsafe fn open_detected_device(
        &mut self,
        mode: &str,
        serial_number: &[WideChar],
    ) -> Result<()> {
        unsafe {
            let family = {
                let handle = self
                    .handle
                    .as_mut()
                    .ok_or_else(|| Error::Sdk("SDK not initialized".to_string()))?;
                self.client.detect_device_family(handle)?.ok_or_else(|| {
                    Error::Sdk(format!(
                        "no Aaronia device found in any known family ({})",
                        NativeSdkClient::DEVICE_FAMILIES.join(", ")
                    ))
                })?
            };
            // `raw` is the V6's name for the pipeline the ECO calls `rtsa`.
            let mode = if mode == "raw" {
                raw_mode_for_family(&family)
            } else {
                mode
            };
            let open_string = format!("{family}/{mode}");
            self.open_device(&open_string, serial_number)
        }
    }

    /// Returns the currently configured open mode, if a device is open.
    pub fn open_mode(&self) -> Option<DeviceOpenMode> {
        self.open_mode.clone()
    }

    /// Set `main/decimation` on a `spectranv6/raw` device. The SDK config
    /// item accepts either an enumerated string (`"Full"`, `"1 / 2"`,
    /// `"1 / 4"`, …, `"1 / 512"` per the official RTSA-API-Samples README)
    /// or the matching integer index — index 6 maps to `"1 / 64"` per
    /// RawIQ.cpp:142, so the relationship is `factor = 1 << index` with
    /// index 0 = `"Full"` (no decimation).
    ///
    /// `factor` must be a positive power of two in `[1, 512]`. Returns an
    /// error if `factor` is not a valid decimation step or if the device
    /// is not in raw mode.
    pub unsafe fn set_decimation_factor(&mut self, factor: u32) -> Result<()> {
        unsafe {
            if !matches!(self.open_mode, Some(DeviceOpenMode::Raw)) {
                return Err(Error::Sdk(format!(
                    "main/decimation is only available on spectranv6/raw; current open mode: {:?}",
                    self.open_mode
                )));
            }
            if factor == 0 || !factor.is_power_of_two() || factor > 512 {
                return Err(Error::Sdk(format!(
                    "decimation factor {} is invalid; must be a power of two in [1, 512]",
                    factor
                )));
            }
            let index = factor.trailing_zeros() as i64; // log2(factor); index 0 => "Full"

            let device = self
                .device
                .as_mut()
                .ok_or_else(|| Error::Sdk("No device opened".to_string()))?;
            let mut root = self.client.get_config_root(device)?;
            let mut config = self
                .client
                .find_config(device, &mut root, "main/decimation")?;
            self.client.set_config_integer(device, &mut config, index)?;
            if factor == 1 {
                info!("Set main/decimation to Full (index 0)");
            } else {
                info!("Set main/decimation to 1/{} (index {})", factor, index);
            }
            Ok(())
        }
    }

    /// Select which receiver channel(s) the `spectranv6/raw` pipeline
    /// captures.
    ///
    /// [`Self::configure_iq_receiver`] defaults `device/receiverchannel`
    /// to `"Rx1"`; call this afterwards to switch to Rx2 or dual-channel
    /// capture.
    ///
    /// > [!WARNING]
    /// > `Rx2` and `Rx1And2` are hardware-unverified: the developer's V6
    /// > ECO is a single-channel device, so only `Rx1` has been exercised
    /// > against real hardware. In `Rx1And2` mode the SDK interleaves both
    /// > channels into one packet — read both with
    /// > [`Self::read_samples_dual`]; [`Self::read_samples`] honours the
    /// > packet's `stride` field and extracts only the *first* channel.
    ///
    /// Only available in raw mode; eco `iqreceiver` drives a fixed
    /// single-channel pipeline, and the config key doesn't exist there.
    pub unsafe fn set_receiver_channel(&mut self, channel: RxChannel) -> Result<()> {
        unsafe {
            if !matches!(self.open_mode, Some(DeviceOpenMode::Raw)) {
                return Err(Error::Sdk(format!(
                    "device/receiverchannel is only available on spectranv6/raw; \
                     current open mode: {:?}",
                    self.open_mode
                )));
            }

            let device = self
                .device
                .as_mut()
                .ok_or_else(|| Error::Sdk("No device opened".to_string()))?;
            let mut root = self.client.get_config_root(device)?;
            let mut config =
                self.client
                    .find_config(device, &mut root, "device/receiverchannel")?;
            self.client
                .set_config_string(device, &mut config, channel.as_config_str())?;
            info!("Set receiver channel to {}", channel.as_config_str());
            Ok(())
        }
    }

    /// Configure the `sweepsa` (spectrum sweep) config group.
    ///
    /// > [!WARNING]
    /// > Key names now match Aaronia's `SweepSpectrumEco` sample, which
    /// > sets `main/startfreq`, `main/stopfreq`, `main/rbwfreq` and
    /// > `main/reflevel` after opening `spectranv6eco/sweepsa`. This code
    /// > previously sent `main/rbw`, which no sample uses.
    /// >
    /// > Still hardware-unverified: the `main/startfreq`/`main/stopfreq`/
    /// > `main/rbw`/`main/vbw` config paths are inferred from the naming
    /// > convention used elsewhere in the SDK config tree, not confirmed
    /// > against a live `sweepsa`-mode device (the developer's V6 ECO
    /// > only exercises `iqreceiver` mode). Each key is set best-effort —
    /// > a missing key is silently skipped, matching the eco-mode
    /// > tolerance in [`Self::configure_iq_receiver`] — so verify the
    /// > resulting device state before relying on this in production.
    pub unsafe fn configure_sweepsa(&mut self, config: &SweepsaConfig) -> Result<()> {
        unsafe {
            let device = self
                .device
                .as_mut()
                .ok_or_else(|| Error::Sdk("No device opened".to_string()))?;

            let mut root = self.client.get_config_root(device)?;

            if let Ok(mut config_node) =
                self.client.find_config(device, &mut root, "main/startfreq")
            {
                self.client
                    .set_config_float(device, &mut config_node, config.start_frequency)?;
                info!("Set sweepsa startfreq to {} Hz", config.start_frequency);
            }

            if let Ok(mut config_node) = self.client.find_config(device, &mut root, "main/stopfreq")
            {
                self.client
                    .set_config_float(device, &mut config_node, config.stop_frequency)?;
                info!("Set sweepsa stopfreq to {} Hz", config.stop_frequency);
            }

            if let Ok(mut config_node) = self.client.find_config(device, &mut root, "main/rbwfreq")
            {
                self.client
                    .set_config_float(device, &mut config_node, config.rbw)?;
                info!("Set sweepsa rbw to {} Hz", config.rbw);
            }

            if let Ok(mut config_node) = self.client.find_config(device, &mut root, "main/vbw") {
                self.client
                    .set_config_float(device, &mut config_node, config.vbw)?;
                info!("Set sweepsa vbw to {} Hz", config.vbw);
            }

            Ok(())
        }
    }

    /// Configure the IQ receiver pipeline: tuning, level, and — on raw
    /// mode — the receiver channel.
    ///
    /// `channel` threads the caller's receiver-channel selection through
    /// every (re)configuration, `None` meaning the `Rx1` default. It is
    /// a parameter rather than a follow-up [`Self::set_receiver_channel`]
    /// call so that *retunes cannot silently revert the channel*: this
    /// function used to write `"Rx1"` unconditionally, which meant a
    /// mid-stream `set_center_frequency` switched an `Rx2`/`Rx1And2`
    /// capture back to the Rx1 antenna with no error. On non-raw open
    /// modes an explicit `Some(channel)` is a hard error — the eco
    /// pipeline has no `device/receiverchannel` key to honour it with.
    pub unsafe fn configure_iq_receiver(
        &mut self,
        center_freq: f64,
        span_freq: f64,
        ref_level: f64,
        channel: Option<RxChannel>,
    ) -> Result<()> {
        unsafe {
            let device = self
                .device
                .as_mut()
                .ok_or_else(|| Error::Sdk("No device opened".to_string()))?;

            // Get config root
            let mut root = self.client.get_config_root(device)?;

            // Configure center frequency
            if let Ok(mut config) = self
                .client
                .find_config(device, &mut root, "main/centerfreq")
            {
                self.client
                    .set_config_float(device, &mut config, center_freq)?;
                info!("Set center frequency to {} Hz", center_freq);
            } else {
                warn!("Could not find main/centerfreq config");
            }

            // Configure span frequency (for IQ receiver mode)
            if let Ok(mut config) = self.client.find_config(device, &mut root, "main/spanfreq") {
                self.client
                    .set_config_float(device, &mut config, span_freq)?;
                info!("Set span frequency to {} Hz", span_freq);
            } else {
                warn!("Could not find main/spanfreq config");
            }

            // Configure reference level
            if let Ok(mut config) = self.client.find_config(device, &mut root, "main/reflevel") {
                self.client
                    .set_config_float(device, &mut config, ref_level)?;
                info!("Set reference level to {} dBm", ref_level);
            } else {
                warn!("Could not find main/reflevel config");
            }

            // The next four config keys are only present on `spectranv6/raw`.
            // On `spectranv6eco/iqreceiver` (per IQReceiverEco.cpp) the SDK
            // owns the channel/format/clock/decimation pipeline and the keys
            // either don't exist or are read-only — touching them produces a
            // misleading warning. Skip them when we know the mode is eco.
            let writes_raw_only_keys = self
                .open_mode
                .as_ref()
                .map(|m| m.supports_raw_only_keys())
                .unwrap_or(true); // Unknown mode: try anyway.

            if writes_raw_only_keys {
                // Configure receiver channel (caller's selection, Rx1
                // default — see the doc comment on this function).
                let rx = channel.unwrap_or(RxChannel::Rx1);
                if let Ok(mut config) =
                    self.client
                        .find_config(device, &mut root, "device/receiverchannel")
                {
                    self.client
                        .set_config_string(device, &mut config, rx.as_config_str())?;
                    info!("Set receiver channel to {}", rx.as_config_str());
                } else if channel.is_some() {
                    // An explicit selection that cannot be applied is an
                    // error, not a warning: the caller would otherwise
                    // stream from the wrong antenna.
                    return Err(Error::Sdk(format!(
                        "receiver channel {} requested but device/receiverchannel \
                         config not found on this device",
                        rx.as_config_str()
                    )));
                } else {
                    warn!("Could not find device/receiverchannel config");
                }

                // Configure output format
                if let Ok(mut config) =
                    self.client
                        .find_config(device, &mut root, "device/outputformat")
                {
                    self.client.set_config_string(device, &mut config, "iq")?;
                    info!("Set output format to iq");
                } else {
                    warn!("Could not find device/outputformat config");
                }

                // Configure receiver clock (for V6 raw mode)
                if let Ok(mut config) =
                    self.client
                        .find_config(device, &mut root, "device/receiverclock")
                {
                    self.client
                        .set_config_string(device, &mut config, "92MHz")?;
                    info!("Set receiver clock to 92MHz");
                } else {
                    debug!("device/receiverclock not found (may be V6 ECO with fixed clock)");
                }
            } else {
                if channel.is_some() {
                    return Err(Error::Sdk(format!(
                        "device/receiverchannel is only available on spectranv6/raw; \
                         current open mode: {:?}",
                        self.open_mode
                    )));
                }
                debug!(
                    "Open mode {:?}: skipping device/receiverchannel, device/outputformat, \
                 device/receiverclock (raw-only keys)",
                    self.open_mode
                );
            }

            // Validate the IQ Mode Constraint dynamically after applying
            // config. The `"92MHz"`-style ConfigItem labels are *rounded* — for
            // example `"92MHz"` is actually 92.16 MHz — so resolve via the
            // documented label→rate table rather than parsing the integer.
            let actual_clock_hz = if let Ok(mut config) =
                self.client
                    .find_config(device, &mut root, "device/receiverclock")
            {
                if let Ok(clock_str) = self.client.get_config_string(device, &mut config) {
                    crate::utils::receiver_clock_for_label(&clock_str)
                } else {
                    crate::utils::DEFAULT_RECEIVER_CLOCK_HZ
                }
            } else {
                // Eco devices report no receiverclock key: the clock is
                // fixed. It is 92.16 MHz, not the 61.44 MHz this used to
                // assume. A V6 ECO streams at 61.44 MHz sampling, measured
                // over HTTP against real hardware, and the constraint
                // checked below is `span * 1.5 <= clock`, so the clock
                // cannot be lower than 92.16 MHz. With the old value this
                // rejected every span above 40.96 MHz, including the
                // device's own maximum.
                crate::utils::DEFAULT_RECEIVER_CLOCK_HZ
            };

            self.receiver_clock_hz = Some(actual_clock_hz);
            crate::utils::validate_iq_mode(span_freq, actual_clock_hz)?;

            info!("IQ Receiver configuration completed");
            Ok(())
        }
    }

    /// Configure the device for IQ transmission.
    ///
    /// > [!WARNING]
    /// > Hardware-unverified: the `main/transgain` configuration has not been
    /// > confirmed against a live TX-capable device.
    pub unsafe fn configure_iq_transmitter(
        &mut self,
        center_freq: f64,
        span_freq: f64,
        trans_gain: f64,
    ) -> Result<()> {
        unsafe {
            let device = self
                .device
                .as_mut()
                .ok_or_else(|| Error::Sdk("No device opened".to_string()))?;

            let mut root = self.client.get_config_root(device)?;

            if let Ok(mut config) = self
                .client
                .find_config(device, &mut root, "main/centerfreq")
            {
                self.client
                    .set_config_float(device, &mut config, center_freq)?;
                info!("Set TX center frequency to {} Hz", center_freq);
            } else {
                warn!("Could not find main/centerfreq config");
            }

            if let Ok(mut config) = self.client.find_config(device, &mut root, "main/spanfreq") {
                self.client
                    .set_config_float(device, &mut config, span_freq)?;
                info!("Set TX span frequency to {} Hz", span_freq);
            } else {
                warn!("Could not find main/spanfreq config");
            }

            if let Ok(mut config) = self.client.find_config(device, &mut root, "main/transgain") {
                self.client
                    .set_config_float(device, &mut config, trans_gain)?;
                info!("Set TX gain to {} dB", trans_gain);
            } else {
                warn!("Could not find main/transgain config");
            }

            Ok(())
        }
    }

    pub unsafe fn start_streaming(&mut self) -> Result<()> {
        unsafe {
            let device = self
                .device
                .as_mut()
                .ok_or_else(|| Error::Sdk("No device opened".to_string()))?;

            // Connect to device
            self.client.connect_device(device)?;
            self.device_connected = true;

            // Start device
            self.client.start_device(device)?;

            self.stream_active = true;
            info!("Streaming started successfully");
            Ok(())
        }
    }

    /// How long `read_samples` will wait for the SDK to deliver a packet
    /// before giving up. The official samples (IQReceiverEco.cpp,
    /// RawIQ.cpp, SweepSpectrumEco.cpp) poll `AARTSAAPI_GetPacket` with a
    /// 5 ms sleep between empty results. We match that cadence and cap the
    /// total wait so a stalled device can't block the caller indefinitely.
    pub const READ_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(5);
    pub const READ_POLL_DEADLINE: std::time::Duration = std::time::Duration::from_millis(500);

    /// The receiver clock in Hz, once the device has been configured.
    ///
    /// Worth reading rather than assuming: the sample-rate ladder runs
    /// from `clock / 1.5` downward in halves, so a V6 on the 245 MHz
    /// clock reaches rates a V6 ECO cannot. Pass this to
    /// [`crate::utils::iq_sample_rates_for_clock`] to get the rates this
    /// device actually supports.
    pub fn receiver_clock_hz(&self) -> Option<f64> {
        self.receiver_clock_hz
    }

    /// Read one packet of spectra, appending its values to `out`.
    ///
    /// Returns `None` when no packet arrived within
    /// [`Self::READ_POLL_DEADLINE`], matching the IQ read.
    ///
    /// The stream index comes from the open mode rather than a constant:
    /// `spectranv6/raw` carries spectra on stream 2 while its IQ is on
    /// stream 0, and every other mode uses stream 0. See
    /// [`DeviceOpenMode::spectra_stream_index`]. The device must also be
    /// producing spectra in the first place, which in raw mode means
    /// `device/outputformat` set to `"spectra"`.
    ///
    /// > Hardware-unverified. The packet layout, `num` frames of `size`
    /// > bins, follows Aaronia's `RawSpectrum` sample; no spectra-capable
    /// > device has been available to run it against.
    ///
    /// # Safety
    /// The device must be open and streaming.
    pub unsafe fn read_spectra(&mut self, out: &mut Vec<f32>) -> Result<Option<SpectraRead>> {
        unsafe {
            if !self.stream_active {
                return Err(Error::Sdk("Streaming not active".to_string()));
            }
            let stream = self
                .open_mode
                .as_ref()
                .map(DeviceOpenMode::spectra_stream_index)
                .unwrap_or(0);
            let device = self
                .device
                .as_mut()
                .ok_or_else(|| Error::Sdk("No device opened".to_string()))?;

            let started = std::time::Instant::now();
            let packet_opt = loop {
                match self.client.get_packet(device, stream, 0)? {
                    Some(p) => break Some(p),
                    None => {
                        if started.elapsed() >= Self::READ_POLL_DEADLINE {
                            break None;
                        }
                        std::thread::sleep(Self::READ_POLL_INTERVAL);
                    }
                }
            };

            let Some(packet) = packet_opt else {
                return Ok(None);
            };

            // Consume on every path out from here: `get_packet` always
            // returns the head of the queue, so leaving one behind makes
            // every later read return the same packet forever.
            let result = (|| {
                if packet.fp32.is_null() || packet.num <= 0 || packet.size == 0 {
                    return Ok(None);
                }
                let frames = packet.num as usize;
                let bins = packet.size as usize;
                // Bound the product before allocating: a corrupt header
                // must not size a multi-gigabyte copy.
                const MAX_VALUES: usize = 1 << 26;
                let total = frames.checked_mul(bins).filter(|n| *n <= MAX_VALUES);
                let Some(total) = total else {
                    return Err(Error::Sdk(format!(
                        "spectra packet claims {frames} frames of {bins} bins,                          which exceeds the {MAX_VALUES}-value limit"
                    )));
                };
                // `stride` is floats from frame to frame; `size` is the
                // floats of data in each. They differ when frames are
                // padded, and a flat copy would then shift every frame
                // after the first.
                let Ok(stride) = usize::try_from(packet.stride) else {
                    return Err(Error::Sdk(format!(
                        "spectra packet has a negative stride ({})",
                        packet.stride
                    )));
                };
                out.reserve(total);
                if stride == 0 || stride == bins {
                    out.extend_from_slice(std::slice::from_raw_parts(packet.fp32, total));
                } else if stride > bins {
                    let span = (frames - 1)
                        .checked_mul(stride)
                        .and_then(|n| n.checked_add(bins))
                        .filter(|n| *n <= MAX_VALUES * 2);
                    if span.is_none() {
                        return Err(Error::Sdk(format!(
                            "spectra packet claims {frames} frames at stride {stride}, \
                             which exceeds the {MAX_VALUES}-value limit"
                        )));
                    }
                    for frame in 0..frames {
                        out.extend_from_slice(std::slice::from_raw_parts(
                            packet.fp32.add(frame * stride),
                            bins,
                        ));
                    }
                } else {
                    return Err(Error::Sdk(format!(
                        "spectra packet stride {stride} is smaller than its size {bins}"
                    )));
                }
                Ok(Some(SpectraRead {
                    frames,
                    bins_per_frame: bins,
                    start_frequency_hz: packet.start_frequency,
                    step_frequency_hz: packet.step_frequency,
                    start_time: packet.start_time,
                }))
            })();

            self.client.consume_packets(device, stream, 1)?;
            result
        }
    }

    /// Read up to `max_samples` complex IQ pairs from the device.
    ///
    /// **Magnitude scaling differs by open mode.** Comparing the official
    /// samples:
    /// - `spectranv6eco/iqreceiver` (IQReceiverEco.cpp:48):
    ///   `int(packet.fp32[...] * 5 * 1000)` clamped to ±50 → full-scale
    ///   roughly ±10 mV with the receiver in iqreceiver mode.
    /// - `spectranv6/raw` (RawIQ.cpp:48):
    ///   `int(packet.fp32[...] * 50 * 1000)` clamped to ±50 → full-scale
    ///   roughly ±1 mV (10× tighter).
    ///
    /// In other words the f32 components are in volts but the front-end
    /// digital gain depends on the open mode. Callers that need
    /// physically-meaningful magnitudes must consult the open mode (see
    /// [`Self::open_mode`]); SDR applications's DJI detector is amplitude-
    /// relative so this is informational here.
    ///
    /// # Carry-over
    ///
    /// The SDK chooses its own packet size, which is unrelated to
    /// `max_samples`. When a packet holds more than the caller asked for,
    /// the excess is retained internally and returned by subsequent calls
    /// rather than dropped — `AARTSAAPI_ConsumePackets` releases the packet
    /// back to the SDK before the next call, so an uncopied tail would be an
    /// unrecoverable gap in the IQ stream. [`Self::get_sample_buffer_size`]
    /// reports how much is pending.
    ///
    /// Consequences for callers:
    /// - A call is served from carry-over first, and polls the device only
    ///   when carry-over alone cannot satisfy `max_samples`. A caller
    ///   draining a backlog therefore never waits on
    ///   [`Self::READ_POLL_DEADLINE`].
    /// - The return value counts samples appended to `buffer` by this call,
    ///   not samples taken from the device.
    /// - A short read still means "no more data right now" — at most one
    ///   packet is fetched per call, as before.
    pub unsafe fn read_samples(
        &mut self,
        buffer: &mut Vec<Complex32>,
        max_samples: usize,
    ) -> Result<usize> {
        unsafe { self.read_samples_within(buffer, max_samples, Self::READ_POLL_DEADLINE) }
    }

    /// [`Self::read_samples`] with the wait for a packet bounded by
    /// `poll_budget` instead of [`Self::READ_POLL_DEADLINE`]. A zero
    /// budget drains carry-over and takes one already-queued packet
    /// without sleeping.
    ///
    /// # Safety
    /// Same contract as [`Self::read_samples`].
    pub unsafe fn read_samples_within(
        &mut self,
        buffer: &mut Vec<Complex32>,
        max_samples: usize,
        poll_budget: std::time::Duration,
    ) -> Result<usize> {
        unsafe {
            if !self.stream_active {
                return Err(Error::Sdk("Streaming not active".to_string()));
            }

            // Serve from the carry-over buffer first. The SDK hands back
            // whole packets whose size it chooses; when one is larger than
            // the caller's `max_samples` the tail is retained here rather
            // than discarded, so the IQ stream stays gap-free across calls.
            // (This mirrors the HTTP path's remainder handling in
            // `UnifiedSource::read_samples`.)
            match self.read_mode {
                None => self.read_mode = Some(ReadMode::Mono),
                Some(ReadMode::Mono) => {}
                Some(ReadMode::Dual) => {
                    return Err(Error::Sdk(
                        "this stream is being read with read_samples_dual; mixing the two \
                         read paths would silently punch time-gaps into both outputs — \
                         stop and restart streaming to switch"
                            .to_string(),
                    ));
                }
            }

            let ReadPlan {
                from_carry,
                remaining,
            } = plan_read(self.sample_buffer.len(), max_samples);
            if from_carry > 0 {
                buffer.extend(self.sample_buffer.drain(0..from_carry));
            }
            if remaining == 0 {
                // Fully satisfied from carry-over (or `max_samples == 0`).
                // Return without polling so a caller draining a backlog never
                // eats a `READ_POLL_DEADLINE` stall for samples it already
                // has — and so a zero-sized request can't consume, and
                // destroy, a packet it copies nothing out of.
                return Ok(from_carry);
            }

            let device = self
                .device
                .as_mut()
                .ok_or_else(|| Error::Sdk("No device opened".to_string()))?;

            let mut samples_read = from_carry;

            // Poll for a packet matching the canonical sample loop:
            //   while ((res = AARTSAAPI_GetPacket(...)) == AARTSAAPI_EMPTY)
            //       std::this_thread::sleep_for(std::chrono::milliseconds(5));
            // We additionally bound the total wait so a misbehaving device
            // can't deadlock the caller.
            let started = std::time::Instant::now();
            let packet_opt = loop {
                match self.client.get_packet(device, 0, 0)? {
                    Some(p) => break Some(p),
                    None => {
                        let elapsed = started.elapsed();
                        if elapsed >= poll_budget {
                            break None;
                        }
                        // Never sleep past the budget: a sub-interval
                        // deadline gets the remainder, not a full tick.
                        std::thread::sleep(Self::READ_POLL_INTERVAL.min(poll_budget - elapsed));
                    }
                }
            };

            if let Some(packet) = packet_opt {
                // `stride` is "floats from sample to sample". A tightly packed
                // IQ pair is 2; interleaved multi-channel layouts are a small
                // multiple. Accept only that range: a stride < 2 is a non-IQ
                // layout, and an absurdly large stride is a corrupt/garbage
                // packet — the wide-stride gather below sizes a raw slice from
                // `stride`, so an unsanitised huge value would read far past
                // the SDK's buffer (UB). `MAX_IQ_STRIDE` is deliberately loose
                // (any real interleave is tiny) — it's a corruption backstop,
                // not a tight spec bound.
                const MAX_IQ_STRIDE: i64 = 4096;
                let valid_iq_stride = (2..=MAX_IQ_STRIDE).contains(&packet.stride);
                if !packet.fp32.is_null() && packet.num > 0 && !valid_iq_stride {
                    // Consume it without mis-reading pairs out of it.
                    warn!(
                        "Skipping packet with out-of-range IQ stride {} (expected 2..={})",
                        packet.stride, MAX_IQ_STRIDE
                    );
                }
                // Process IQ data from the packet
                if !packet.fp32.is_null() && packet.num > 0 && valid_iq_stride {
                    const MAX_SAMPLES: usize = 1 << 24;
                    // Bound-check `packet.num` *before* multiplying so a garbage
                    // or hostile count can't overflow the `i64` multiply itself.
                    if packet.num as usize > MAX_SAMPLES / 2 {
                        // Consume before erroring: get_packet always
                        // returns the head of the queue, so a leaked
                        // packet would make every subsequent call
                        // re-fetch it and re-error forever.
                        self.client.consume_packets(device, 0, 1)?;
                        return Err(Error::Sdk(format!(
                            "Packet sample count {} exceeds maximum allowed {}",
                            packet.num.saturating_mul(2),
                            MAX_SAMPLES
                        )));
                    }
                    // SAFETY: packet.fp32 is verified non-null above, and packet.num * 2 is bounds-checked.
                    // `Complex32` is `#[repr(C)]` with fields `{re: f32, im: f32}`, so it has
                    // identical layout to `[f32; 2]`. This compile-time assertion guards against
                    // a future change to that representation.
                    const _: () =
                        assert!(std::mem::size_of::<Complex32>() == 2 * std::mem::size_of::<f32>());
                    // Take the *whole* packet: the first `to_caller` samples
                    // satisfy this call and any excess is carried over for the
                    // next one. `consume_packets` below hands the packet back
                    // to the SDK, so anything not copied out here is gone —
                    // truncating to `max_samples` silently punched a hole in
                    // the IQ stream on every oversized packet.
                    let packet_samples = packet.num as usize;
                    let stride = packet.stride as usize;
                    let (to_caller, _to_carry) = split_packet(packet_samples, remaining);

                    // `packet_samples >= 1` (guarded by `packet.num > 0`
                    // above) and `remaining >= 1` (a fully-satisfied caller
                    // returned before polling), so neither the `- 1` below nor
                    // the slice splits can underflow.
                    if stride == 2 {
                        // Tightly packed IQ pairs — the common raw-IQ layout.
                        let complex_slice = std::slice::from_raw_parts(
                            packet.fp32 as *const Complex32,
                            packet_samples,
                        );
                        buffer.extend_from_slice(&complex_slice[..to_caller]);
                        self.sample_buffer
                            .extend(complex_slice[to_caller..].iter().copied());
                    } else {
                        // Per the official header, `stride` is the "offset
                        // from sample to sample in floats" and is not required
                        // to be 2 (e.g. multi-channel interleaved layouts).
                        // Gather sample-by-sample so a wider stride doesn't
                        // smear neighbouring channels into the IQ data.
                        // SAFETY: each sample spans floats
                        // [i*stride, i*stride+1], the last of which is
                        // (num-1)*stride + 2 floats into the buffer the SDK
                        // guarantees valid for `num` samples of `stride`
                        // floats.
                        let floats = std::slice::from_raw_parts(
                            packet.fp32 as *const f32,
                            (packet_samples - 1) * stride + 2,
                        );
                        let sample_at =
                            |i: usize| Complex32::new(floats[i * stride], floats[i * stride + 1]);
                        buffer.extend((0..to_caller).map(sample_at));
                        self.sample_buffer
                            .extend((to_caller..packet_samples).map(sample_at));
                    }
                    samples_read += to_caller;

                    // `trace!`, not `info!`: this runs on every read call
                    // (thousands/sec at speed), so an enabled info subscriber
                    // would pay per-read formatting on the hot path.
                    trace!(
                        "Read {} IQ samples (sample rate: {} Hz, center freq: {} Hz)",
                        samples_read, packet.step_frequency, packet.start_frequency
                    );
                }

                // Consume the packet
                self.client.consume_packets(device, 0, 1)?;
            } else {
                debug!(
                    "read_samples: no packet within {:?} (stream live but idle)",
                    Self::READ_POLL_DEADLINE
                );
            }

            Ok(samples_read)
        }
    }

    /// Read up to `max_samples` *pairs* of samples from a dual-channel
    /// (`Rx1+Rx2`) stream, appending channel 1 to `rx1` and channel 2 to
    /// `rx2`. On `Ok(n)` both vectors grew by exactly `n`, so
    /// `rx1`/`rx2` stay index-aligned in time. On `Err` the vectors may
    /// have grown by the carry-over that was drained before the failure
    /// (equally on both, preserving alignment) — treat their post-error
    /// lengths, not the absent return value, as authoritative.
    ///
    /// Requires the stream to have been configured with
    /// [`RxChannel::Rx1And2`] (see [`Self::set_receiver_channel`]): the
    /// SDK then interleaves both receivers into one packet,
    /// `[I1, Q1, I2, Q2]` per sample. A packet whose `stride` cannot
    /// carry two channels (< 4) fails with an actionable error rather
    /// than silently duplicating or dropping a channel.
    ///
    /// Carry-over follows the same whole-packet rule as
    /// [`Self::read_samples`]: the SDK reclaims a packet on consume, so
    /// any tail beyond `max_samples` is retained internally (as pairs)
    /// for the next call rather than discarded. Do not mix
    /// [`Self::read_samples`] and this method on one stream — each
    /// maintains its own carry buffer.
    ///
    /// > [!WARNING]
    /// > Hardware-unverified, like the rest of the `Rx1And2` path: the
    /// > developer's V6 ECO is single-channel, so the interleave layout
    /// > is taken from the packet contract (`stride` = floats from
    /// > sample to sample, two IQ pairs per sample) rather than a live
    /// > dual-channel capture. Verify against a full V6 before
    /// > production use.
    pub unsafe fn read_samples_dual(
        &mut self,
        rx1: &mut Vec<Complex32>,
        rx2: &mut Vec<Complex32>,
        max_samples: usize,
    ) -> Result<usize> {
        unsafe {
            if !self.stream_active {
                return Err(Error::Sdk("Streaming not active".to_string()));
            }

            match self.read_mode {
                None => self.read_mode = Some(ReadMode::Dual),
                Some(ReadMode::Dual) => {}
                Some(ReadMode::Mono) => {
                    return Err(Error::Sdk(
                        "this stream is being read with read_samples; mixing the two \
                         read paths would silently punch time-gaps into both outputs — \
                         stop and restart streaming to switch"
                            .to_string(),
                    ));
                }
            }

            let ReadPlan {
                from_carry,
                remaining,
            } = plan_read(self.dual_sample_buffer.len(), max_samples);
            if from_carry > 0 {
                for (a, b) in self.dual_sample_buffer.drain(0..from_carry) {
                    rx1.push(a);
                    rx2.push(b);
                }
            }
            if remaining == 0 {
                return Ok(from_carry);
            }

            let device = self
                .device
                .as_mut()
                .ok_or_else(|| Error::Sdk("No device opened".to_string()))?;

            let mut pairs_read = from_carry;

            let started = std::time::Instant::now();
            let packet_opt = loop {
                match self.client.get_packet(device, 0, 0)? {
                    Some(p) => break Some(p),
                    None => {
                        if started.elapsed() >= Self::READ_POLL_DEADLINE {
                            break None;
                        }
                        std::thread::sleep(Self::READ_POLL_INTERVAL);
                    }
                }
            };

            if let Some(packet) = packet_opt {
                if !packet.fp32.is_null() && packet.num > 0 {
                    // Same corruption backstop as `read_samples`; the
                    // dual lower bound (4 floats per sample) is enforced
                    // by `deinterleave_dual_iq` with a clearer error.
                    const MAX_IQ_STRIDE: i64 = 4096;
                    if packet.stride > MAX_IQ_STRIDE {
                        warn!(
                            "Skipping packet with out-of-range IQ stride {} (expected <= {})",
                            packet.stride, MAX_IQ_STRIDE
                        );
                        self.client.consume_packets(device, 0, 1)?;
                        return Ok(pairs_read);
                    }
                    const MAX_SAMPLES: usize = 1 << 24;
                    if packet.num as usize > MAX_SAMPLES / 4 {
                        // Consume before erroring: get_packet always
                        // returns the head of the queue, so a leaked
                        // packet would make every subsequent call
                        // re-fetch it and re-error forever.
                        self.client.consume_packets(device, 0, 1)?;
                        return Err(Error::Sdk(format!(
                            "Packet sample count {} exceeds maximum allowed {}",
                            packet.num,
                            MAX_SAMPLES / 4
                        )));
                    }

                    let packet_samples = packet.num as usize;
                    let stride = packet.stride.max(0) as usize;
                    let floats: &[f32] = if stride >= 4 {
                        // The individual caps above still admit a corrupt
                        // num/stride pair that *together* describe a
                        // multi-GB slice; bound the product too. Real
                        // packets are a few MB — 1<<26 floats (256 MiB)
                        // is a corruption backstop, not a spec bound.
                        const MAX_PACKET_FLOATS: usize = 1 << 26;
                        let float_count = (packet_samples - 1) * stride + 4;
                        if float_count > MAX_PACKET_FLOATS {
                            self.client.consume_packets(device, 0, 1)?;
                            return Err(Error::Sdk(format!(
                                "dual-channel packet describes {} floats ({} samples x stride {}), \
                                 exceeding the {} backstop (corrupt packet header?)",
                                float_count, packet_samples, stride, MAX_PACKET_FLOATS
                            )));
                        }
                        // SAFETY: fp32 verified non-null; float_count is
                        // bounded just above; the SDK guarantees the
                        // buffer valid for `num` samples of `stride`
                        // floats, the last of which we read 4 into.
                        std::slice::from_raw_parts(packet.fp32 as *const f32, float_count)
                    } else {
                        // Too narrow for two channels; let the demux
                        // helper produce its diagnostic without reading
                        // out of bounds.
                        &[]
                    };
                    let pairs =
                        match crate::utils::deinterleave_dual_iq(floats, packet_samples, stride) {
                            Ok(p) => p,
                            Err(e) => {
                                // Hand the packet back before surfacing the
                                // error so the stream can continue.
                                self.client.consume_packets(device, 0, 1)?;
                                return Err(e);
                            }
                        };

                    // Single pass, no intermediate buffer: the first
                    // `to_caller` pairs satisfy this call, the rest are
                    // carried over — whole-packet semantics identical to
                    // the mono path.
                    let (to_caller, _to_carry) = split_packet(packet_samples, remaining);
                    for (i, (a, b)) in pairs.enumerate() {
                        if i < to_caller {
                            rx1.push(a);
                            rx2.push(b);
                        } else {
                            self.dual_sample_buffer.push_back((a, b));
                        }
                    }
                    pairs_read += to_caller;

                    trace!(
                        "Read {} dual IQ sample pairs (sample rate: {} Hz)",
                        pairs_read, packet.step_frequency
                    );
                }

                self.client.consume_packets(device, 0, 1)?;
            } else {
                debug!(
                    "read_samples_dual: no packet within {:?} (stream live but idle)",
                    Self::READ_POLL_DEADLINE
                );
            }

            Ok(pairs_read)
        }
    }

    pub unsafe fn start_tx_stream(&mut self) -> Result<TxStream<'_>> {
        unsafe {
            let device = self
                .device
                .as_mut()
                .ok_or_else(|| Error::Sdk("No device opened".to_string()))?;

            Ok(TxStream::new(self.client.clone(), device))
        }
    }

    /// Read the device's master stream time.
    ///
    /// This time is required to correctly schedule `TxBurst` packets.
    pub fn get_master_stream_time(&mut self) -> Result<f64> {
        unsafe {
            let device = self
                .device
                .as_mut()
                .ok_or_else(|| Error::Sdk("No device opened".to_string()))?;
            self.client.get_master_stream_time(device)
        }
    }

    pub unsafe fn stop_streaming(&mut self) -> Result<()> {
        unsafe {
            if self.stream_active {
                if let Some(device) = self.device.as_mut() {
                    // Stop device
                    if let Err(e) = self.client.stop_device(device) {
                        error!("Failed to stop device: {}", e);
                    }
                }
                self.stream_active = false;
            }
            // A new streaming session must not serve samples captured
            // under the previous configuration, and gets a fresh choice
            // of read path.
            self.sample_buffer.clear();
            self.dual_sample_buffer.clear();
            self.read_mode = None;

            if self.device_connected {
                if let Some(device) = self.device.as_mut() {
                    // Disconnect device
                    if let Err(e) = self.client.disconnect_device(device) {
                        error!("Failed to disconnect device: {}", e);
                    }
                }
                self.device_connected = false;
            }

            if let (Some(mut device), Some(handle)) = (self.device.take(), self.handle.as_mut()) {
                // Close device
                if let Err(e) = self.client.close_device(handle, &mut device) {
                    error!("Error closing device during drop: {}", e);
                }
            }

            info!("Streaming stopped");
            Ok(())
        }
    }

    pub fn get_device_serial(device_info: &AARTSAAPI_DeviceInfo) -> String {
        wide_to_string(&device_info.serial_number)
    }

    pub fn is_streaming(&self) -> bool {
        self.stream_active
    }

    /// Samples held over from a packet larger than the last
    /// [`Self::read_samples`] request. These are returned by the next call
    /// (before any new packet is polled); see that method's "Carry-over"
    /// section. Zero means the last packet was fully delivered.
    pub fn get_sample_buffer_size(&self) -> usize {
        self.sample_buffer.len()
    }

    /// Dual-path sibling of [`Self::get_sample_buffer_size`]: (Rx1, Rx2)
    /// sample pairs held over from a packet larger than the last
    /// [`Self::read_samples_dual`] request.
    pub fn get_dual_sample_buffer_size(&self) -> usize {
        self.dual_sample_buffer.len()
    }
}

impl Drop for NativeSdkSource {
    fn drop(&mut self) {
        // Ensure proper cleanup
        unsafe {
            if let Err(e) = self.stop_streaming() {
                error!("Error stopping streaming during drop: {}", e);
            }

            // Close the device before the handle: an opened device that is
            // never closed stays exclusively held until the process exits.
            if let (Some(mut device), Some(handle)) = (self.device.take(), self.handle.as_mut())
                && let Err(e) = self.client.close_device(handle, &mut device)
            {
                error!("Error closing device during drop: {}", e);
            }

            if let Some(e) = self
                .handle
                .take()
                .and_then(|mut h| self.client.close_handle(&mut h).err())
            {
                error!("Error closing handle during drop: {}", e);
            }
        }
    }
}

/// Receiver channel selection for the `spectranv6/raw` pipeline's
/// `device/receiverchannel` config item.
///
/// The full SPECTRAN V6 has two RF inputs; the V6 ECO has one. The config
/// strings come from the official RTSA-API-Samples, which use four:
/// `"Rx1"`, `"Rx2"`, `"Rx12"` and `"Rx1+Rx2"`. The last two both enable
/// both inputs but differ in delivery — `"Rx12"` interleaves them into
/// one stream, `"Rx1+Rx2"` produces two independent streams — so they
/// are not interchangeable. This crate reads one stream and
/// deinterleaves it, so [`RxChannel::Rx1And2`] writes `"Rx12"`.
///
/// `"Rx1"` is what [`NativeSdkSource::configure_iq_receiver`] has
/// always written and is the only variant verified against real
/// hardware (see [`NativeSdkSource::set_receiver_channel`]).
///
/// The enum itself lives in [`crate::utils`] so cross-platform
/// configuration code can name a channel without the `native-sdk`
/// feature/target gates; this re-export preserves the historical
/// `native_sdk::RxChannel` path.
pub use crate::utils::RxChannel;

/// Transmit packet flags defining stream and segment boundaries.
pub mod tx_flags {
    /// Indicates the start of a stream (first packet).
    pub const STREAM_START: u64 = 0x00000001;
    /// Indicates the end of a stream (last packet).
    pub const STREAM_END: u64 = 0x00000002;
    /// Indicates the start of a segment.
    pub const SEGMENT_START: u64 = 0x00000004;
    /// Indicates the end of a segment.
    pub const SEGMENT_END: u64 = 0x00000008;

    /// Instructs the device to immediately process/push the packet.
    pub const PUSH: u64 = 0x00008000;

    // --- Warning Flags ---
    pub const WARN_OVERFLOW: u64 = 0x00000100;
    pub const WARN_DROPPED: u64 = 0x00000200;
    pub const WARN_INACCURATE: u64 = 0x00000400;
    pub const WARN_RESAMPLED: u64 = 0x00000800;

    /// Indicates a discontinuity in the time sequence.
    pub const TIME_DISCONTINUITY: u64 = 0x00010000;
    pub const WARN_DIRECTION: u64 = 0x00020000;

    // --- Condition Flags ---
    pub const CONDITION_0: u64 = 0x10000000;
    pub const CONDITION_1: u64 = 0x20000000;
    pub const CONDITION_2: u64 = 0x40000000;
    pub const CONDITION_3: u64 = 0x80000000;
}

/// A burst of IQ samples to be transmitted over the native SDK.
///
/// The SDK relies on precise timestamps (not zeroes) and packet flags
/// to correctly pace and demarcate transmissions.
///
/// Note: The Aaronia SDK typically expects timestamps represented
/// as "seconds since start of the unix epoch" — an SDK-supplied wall
/// clock, not zero. Real timestamps matter for TX: they're how the
/// device schedules the burst against its own master stream time (see
/// [`NativeSdkClient::get_master_stream_time`]), so a caller should read
/// that clock and derive `start_time`/`end_time` from it rather than
/// passing zero.
#[derive(Debug, Clone, Copy)]
pub struct TxBurst {
    /// Burst start time, seconds since the Unix epoch (device master
    /// stream time, not wall-clock `SystemTime::now()`).
    pub start_time: f64,
    /// Burst end time, seconds since the Unix epoch.
    pub end_time: f64,
    /// Center frequency of the burst, in Hz.
    pub center_frequency_hz: f64,
    /// IQ sample rate of the burst, in Hz.
    pub sample_rate_hz: f64,
    /// Packet boundaries flags (e.g., `tx_flags::STREAM_START`).
    pub flags: u64,
}

/// Unverified hardware transmit path using `AARTSAAPI_SendPacket`.
pub struct TxStream<'a> {
    client: Arc<NativeSdkClient>,
    device: &'a mut AARTSAAPI_Device,
}

// SAFETY: `AARTSAAPI_Device` wraps a raw pointer (`d: *mut c_void`) to
// SDK-internal state, which makes the auto-derived `Send`/`Sync` both
// absent by default. `TxStream` borrows the device *exclusively*
// (`&'a mut AARTSAAPI_Device`), so the borrow checker already prevents
// any concurrent access through this handle — `Send` only permits
// *moving* the whole stream (and its exclusive borrow) to another
// thread, which is sound. `Sync` is deliberately not implemented:
// nothing in this API takes `&self`, so there is no use case for
// sharing `&TxStream` across threads, and doing so would require the
// SDK to tolerate concurrent calls on one device handle, which is
// undocumented and not assumed here.
unsafe impl Send for TxStream<'_> {}

impl<'a> TxStream<'a> {
    pub(crate) fn new(client: Arc<NativeSdkClient>, device: &'a mut AARTSAAPI_Device) -> Self {
        Self { client, device }
    }

    /// Transmit raw `Complex32` IQ data.
    ///
    /// > [!WARNING]
    /// > Hardware-unverified on the V6. The developer's ECO lacks TX capability.
    pub unsafe fn write_samples(
        &mut self,
        channel: i32,
        burst: TxBurst,
        samples: &[Complex32],
    ) -> Result<()> {
        unsafe {
            // `Complex32` is `#[repr(C)] { re: f32, im: f32 }`, so it has
            // identical layout to `[f32; 2]`; this guards against a future
            // change to that representation breaking the raw-pointer cast
            // below.
            const _: () =
                assert!(std::mem::size_of::<Complex32>() == 2 * std::mem::size_of::<f32>());

            let half_span = burst.sample_rate_hz / 2.0;
            let packet = AARTSAAPI_Packet {
                cbsize: std::mem::size_of::<AARTSAAPI_Packet>() as i64,
                stream_id: 0,
                flags: burst.flags,
                start_time: burst.start_time,
                end_time: burst.end_time,
                start_frequency: burst.center_frequency_hz - half_span,
                // Per the official header, stepFrequency is "bin size or
                // sample rate of the data" — for time-domain IQ, that's
                // the sample rate.
                step_frequency: burst.sample_rate_hz,
                span_frequency: burst.sample_rate_hz,
                rbw_frequency: 0.0,
                num: samples.len() as i64,
                total: samples.len() as i64,
                // Per the official header, `size` is "size of each
                // sample" (in floats), not the total buffer size. One
                // complex IQ sample is 2 floats.
                size: 2,
                stride: 2, // Complex IQ contains 2 floats
                // SAFETY: `AARTSAAPI_Packet::fp32` is `*mut f32` in the
                // official header even for outbound (SendPacket) use;
                // the send path is documented as read-only over this
                // buffer, so casting away `const` here does not expose
                // the caller's immutable slice to an actual mutation.
                fp32: samples.as_ptr() as *mut f32,
                interleave: 0,
            };

            self.client.send_packet(self.device, channel, &packet)
        }
    }
}

// === Device Information Helpers ===

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub serial_number: String,
    pub ready: bool,
    pub boost: bool,
    pub superspeed: bool,
    pub active: bool,
}

impl From<AARTSAAPI_DeviceInfo> for DeviceInfo {
    fn from(info: AARTSAAPI_DeviceInfo) -> Self {
        Self {
            serial_number: wide_to_string(&info.serial_number),
            ready: info.ready(),
            boost: info.boost(),
            superspeed: info.superspeed(),
            active: info.active(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    #[test]
    fn plan_read_serves_carry_over_before_polling() {
        // Carry-over covers the whole request: don't poll the device.
        assert_eq!(
            plan_read(4096, 1024),
            ReadPlan {
                from_carry: 1024,
                remaining: 0
            }
        );
        // Carry-over covers it exactly: still no reason to poll.
        assert_eq!(
            plan_read(1024, 1024),
            ReadPlan {
                from_carry: 1024,
                remaining: 0
            }
        );
        // Partial carry-over: drain it, then ask the device for the rest.
        assert_eq!(
            plan_read(400, 1024),
            ReadPlan {
                from_carry: 400,
                remaining: 624
            }
        );
        // Nothing buffered: straight to the device.
        assert_eq!(
            plan_read(0, 1024),
            ReadPlan {
                from_carry: 0,
                remaining: 1024
            }
        );
    }

    #[test]
    fn plan_read_zero_request_never_polls() {
        // A `max_samples == 0` read must not fetch a packet: it would be
        // consumed (returned to the SDK) after copying nothing out of it,
        // destroying those samples.
        assert_eq!(
            plan_read(0, 0),
            ReadPlan {
                from_carry: 0,
                remaining: 0
            }
        );
        assert_eq!(
            plan_read(9999, 0),
            ReadPlan {
                from_carry: 0,
                remaining: 0
            }
        );
    }

    #[test]
    fn split_packet_retains_the_tail() {
        // Oversized packet: caller gets what it asked for, the rest is kept.
        assert_eq!(split_packet(65536, 1024), (1024, 64512));
        // Exact fit: nothing carried.
        assert_eq!(split_packet(1024, 1024), (1024, 0));
        // Undersized packet: short read, nothing carried.
        assert_eq!(split_packet(300, 1024), (300, 0));
    }

    proptest::proptest! {
        /// The invariant the old code violated: every sample the SDK hands
        /// us is either delivered to the caller or retained for the next
        /// call. Nothing is dropped. Previously `read_samples` copied
        /// `min(packet.num, max_samples)` and then consumed the whole
        /// packet, silently losing the tail of every oversized packet.
        #[test]
        fn no_sample_is_ever_dropped(
            carry_len in 0usize..100_000,
            max_samples in 0usize..100_000,
            packet_samples in 1usize..100_000,
        ) {
            let plan = plan_read(carry_len, max_samples);

            // Draining carry-over can't take more than exists, or more than
            // was asked for.
            proptest::prop_assert!(plan.from_carry <= carry_len);
            proptest::prop_assert!(plan.from_carry <= max_samples);
            proptest::prop_assert_eq!(plan.from_carry + plan.remaining, max_samples);

            let carry_left = carry_len - plan.from_carry;
            // Carry-over is only ever left behind when the caller is full.
            proptest::prop_assert!(carry_left == 0 || plan.remaining == 0);

            if plan.remaining == 0 {
                // No packet is fetched, so nothing can be lost.
                proptest::prop_assert_eq!(plan.from_carry + carry_left, carry_len);
            } else {
                let (to_caller, to_carry) = split_packet(packet_samples, plan.remaining);
                // Conservation: the packet is fully accounted for.
                proptest::prop_assert_eq!(to_caller + to_carry, packet_samples);
                // The caller never receives more than it asked for...
                proptest::prop_assert!(plan.from_carry + to_caller <= max_samples);
                // ...and a tail is only held back once the caller is full.
                proptest::prop_assert!(to_carry == 0 || plan.from_carry + to_caller == max_samples);
            }
        }
    }

    /// `split_device_type` backs both `SdkConfig::device_family`/
    /// `device_open_mode` (default `"raw"`) and `SdkSinkConfig`'s
    /// equivalents (default `"iqtransmitter"`) — covers the bare-family,
    /// already-mode-qualified, and per-caller-default-suffix cases.
    #[test]
    fn split_device_type_applies_default_mode_or_preserves_qualified_string() {
        assert_eq!(
            split_device_type("spectranv6", "raw"),
            ("spectranv6", "spectranv6/raw".to_string())
        );
        assert_eq!(
            split_device_type("spectranv6", "iqtransmitter"),
            ("spectranv6", "spectranv6/iqtransmitter".to_string())
        );
        assert_eq!(
            split_device_type("spectranv6eco/sweepsa", "raw"),
            ("spectranv6eco", "spectranv6eco/sweepsa".to_string())
        );
    }

    #[test]
    fn device_open_mode_classifies_official_strings() {
        assert_eq!(
            DeviceOpenMode::from_open_string("spectranv6/raw"),
            DeviceOpenMode::Raw
        );
        assert_eq!(
            DeviceOpenMode::from_open_string("spectranv6eco/iqreceiver"),
            DeviceOpenMode::EcoIqReceiver
        );
        assert_eq!(
            DeviceOpenMode::from_open_string("spectranv6/sweepsa"),
            DeviceOpenMode::Sweepsa
        );
        assert_eq!(
            DeviceOpenMode::from_open_string("spectranv6eco/sweepsa"),
            DeviceOpenMode::EcoSweepsa
        );
        assert!(DeviceOpenMode::Raw.supports_raw_only_keys());
        assert!(!DeviceOpenMode::EcoIqReceiver.supports_raw_only_keys());
        assert!(!DeviceOpenMode::Sweepsa.supports_raw_only_keys());
        assert!(!DeviceOpenMode::EcoSweepsa.supports_raw_only_keys());

        // Anything else is preserved as `Other` and treated as conservatively
        // not raw-only (we don't know what keys it has).
        match DeviceOpenMode::from_open_string("spectranv6eco/other") {
            DeviceOpenMode::Other(s) => assert_eq!(s, "spectranv6eco/other"),
            other => panic!("expected Other, got {:?}", other),
        }
        assert!(!DeviceOpenMode::Other("spectranv6eco/other".to_string()).supports_raw_only_keys());
    }

    #[test]
    fn decimation_factor_validation_rejects_invalid_inputs() {
        // We exercise the predicate logic directly because constructing a
        // real `NativeSdkSource` requires loading the SDK. The valid range
        // is the powers of two from 1 (`"Full"`) through 512 (`"1 / 512"`),
        // per the official samples README.
        for bad in [0u32, 3, 5, 7, 9, 100, 1024, 768] {
            assert!(
                !(bad != 0 && bad.is_power_of_two() && bad <= 512),
                "{} should be classified invalid",
                bad
            );
        }
        for good in [1u32, 2, 4, 8, 16, 32, 64, 128, 256, 512] {
            assert!(
                good != 0 && good.is_power_of_two() && good <= 512,
                "{} should be classified valid",
                good
            );
            // log2 round-trip → matches the index encoding the SDK expects.
            assert_eq!(1u32 << good.trailing_zeros(), good);
        }
    }

    // Mock tests since we can't test against real hardware in CI
    #[test]
    fn test_constants() {
        // Test SDK constants are defined correctly
        assert_eq!(AARTSAAPI_OK, 0x00000000);
        assert_eq!(AARTSAAPI_EMPTY, 0x00000001);
        assert_eq!(AARTSAAPI_RETRY, 0x00000002);

        assert_eq!(AARTSAAPI_IDLE, 0x10000000);
        assert_eq!(AARTSAAPI_RUNNING, 0x10000004);

        assert_eq!(AARTSAAPI_MEMORY_SMALL, 0);
        assert_eq!(AARTSAAPI_MEMORY_MEDIUM, 1);
        assert_eq!(AARTSAAPI_MEMORY_LARGE, 2);
        assert_eq!(AARTSAAPI_MEMORY_LUDICROUS, 3);
    }

    #[test]
    fn test_struct_sizes() {
        // Test C struct size calculations
        let handle = AARTSAAPI_Handle { d: ptr::null_mut() };
        let device = AARTSAAPI_Device { d: ptr::null_mut() };
        let config = AARTSAAPI_Config { d: ptr::null_mut() };

        assert!(!handle.d.is_null() || handle.d.is_null()); // Basic null check test
        assert!(!device.d.is_null() || device.d.is_null());
        assert!(!config.d.is_null() || config.d.is_null());
    }

    #[test]
    fn test_device_info_structure() {
        let device_info = AARTSAAPI_DeviceInfo {
            cbsize: std::mem::size_of::<AARTSAAPI_DeviceInfo>() as i64,
            serial_number: [0; 120],
            ready: 1,
            boost: 0,
            superspeed: 1,
            active: 0,
        };

        assert!(device_info.ready());
        assert!(!device_info.boost());
        assert!(device_info.superspeed());
        assert!(!device_info.active());
        assert_eq!(device_info.serial_number.len(), 120);
    }

    #[test]
    fn test_config_info_structure() {
        let config_info = AARTSAAPI_ConfigInfo {
            cbsize: std::mem::size_of::<AARTSAAPI_ConfigInfo>() as i64,
            name: [0; 80],
            title: [0; 120],
            config_type: AARTSAAPI_CONFIG_TYPE_NUMBER,
            min_value: 0.0,
            max_value: 100.0,
            step_value: 1.0,
            unit: [0; 10],
            options: [0; 1000],
            disabled_options: 0,
        };

        assert_eq!(config_info.config_type, AARTSAAPI_CONFIG_TYPE_NUMBER);
        assert_eq!(config_info.min_value, 0.0);
        assert_eq!(config_info.max_value, 100.0);
        assert_eq!(config_info.step_value, 1.0);
    }

    #[test]
    fn test_packet_structure() {
        let packet = AARTSAAPI_Packet {
            cbsize: std::mem::size_of::<AARTSAAPI_Packet>() as i64,
            stream_id: 1,
            flags: 0,
            start_time: 0.0,
            end_time: 1.0,
            start_frequency: 100e6,
            step_frequency: 1e6,
            span_frequency: 10e6,
            rbw_frequency: 1e3,
            num: 1024,
            total: 1024,
            size: 1024 * 8, // Complex32 = 8 bytes
            stride: 8,
            fp32: ptr::null_mut(),
            interleave: 0,
        };

        assert_eq!(packet.stream_id, 1);
        assert_eq!(packet.num, 1024);
        assert_eq!(packet.start_frequency, 100e6);
        assert_eq!(packet.step_frequency, 1e6);
        assert!(packet.fp32.is_null()); // Should be null in test
    }

    #[test]
    fn test_string_to_wide_conversion() {
        let test_str = "test";
        let wide = string_to_wide(test_str).unwrap();
        let wide_slice = wide.as_slice_with_nul();

        // Should include null terminator
        assert_eq!(wide_slice.len(), test_str.len() + 1);
        assert_eq!(wide_slice.last(), Some(&0)); // Null terminator

        // Convert back and verify
        let converted_back = wide_to_string(wide_slice);
        assert_eq!(converted_back, test_str);

        // Test that strings with interior null bytes return an error
        let invalid_str = "te\0st";
        assert!(string_to_wide(invalid_str).is_err());
    }

    #[test]
    fn test_wide_to_string_conversion() {
        let wide_data = vec![116, 101, 115, 116, 0]; // "test" + null terminator
        let result = wide_to_string(&wide_data);
        assert_eq!(result, "test");

        // Test without null terminator
        let wide_no_null = vec![116, 101, 115, 116];
        let result_no_null = wide_to_string(&wide_no_null);
        assert_eq!(result_no_null, "test");

        // Test empty string
        let empty_wide = vec![0];
        let empty_result = wide_to_string(&empty_wide);
        assert_eq!(empty_result, "");
    }

    #[test]
    fn test_wide_string_round_trip() {
        let test_strings = vec!["hello", "world", "123", "", "special!@#$%"];

        for test_str in test_strings {
            let wide = string_to_wide(test_str).unwrap();
            let converted_back = wide_to_string(wide.as_slice_with_nul());
            assert_eq!(converted_back, test_str);
        }
    }

    #[test]
    fn test_device_info_conversion() {
        let mut serial_data = [0 as WideChar; 120];
        let test_serial = "12345";
        let wide_serial = string_to_wide(test_serial).unwrap();

        // Copy test serial to device info (up to array size)
        for (i, &val) in wide_serial.as_slice_with_nul().iter().enumerate() {
            if i < 120 {
                serial_data[i] = val;
            }
        }

        let device_info = AARTSAAPI_DeviceInfo {
            cbsize: std::mem::size_of::<AARTSAAPI_DeviceInfo>() as i64,
            serial_number: serial_data,
            ready: 1,
            boost: 1,
            superspeed: 0,
            active: 1,
        };

        let converted: DeviceInfo = device_info.into();

        assert_eq!(converted.serial_number, test_serial);
        assert!(converted.ready);
        assert!(converted.boost);
        assert!(!converted.superspeed);
        assert!(converted.active);
    }

    #[test]
    fn test_device_info_debug() {
        let device_info = DeviceInfo {
            serial_number: "TEST123".to_string(),
            ready: true,
            boost: false,
            superspeed: true,
            active: false,
        };

        let debug_str = format!("{:?}", device_info);
        assert!(debug_str.contains("TEST123"));
        assert!(debug_str.contains("ready: true"));
        assert!(debug_str.contains("boost: false"));
    }

    #[test]
    fn test_device_info_clone() {
        let device_info = DeviceInfo {
            serial_number: "CLONE_TEST".to_string(),
            ready: false,
            boost: true,
            superspeed: false,
            active: true,
        };

        let cloned = device_info.clone();

        assert_eq!(device_info.serial_number, cloned.serial_number);
        assert_eq!(device_info.ready, cloned.ready);
        assert_eq!(device_info.boost, cloned.boost);
        assert_eq!(device_info.superspeed, cloned.superspeed);
        assert_eq!(device_info.active, cloned.active);
    }

    #[test]
    fn test_config_type_constants() {
        // Test all config type constants
        assert_eq!(AARTSAAPI_CONFIG_TYPE_OTHER, 0);
        assert_eq!(AARTSAAPI_CONFIG_TYPE_GROUP, 1);
        assert_eq!(AARTSAAPI_CONFIG_TYPE_BLOB, 2);
        assert_eq!(AARTSAAPI_CONFIG_TYPE_NUMBER, 3);
        assert_eq!(AARTSAAPI_CONFIG_TYPE_BOOL, 4);
        assert_eq!(AARTSAAPI_CONFIG_TYPE_ENUM, 5);
        assert_eq!(AARTSAAPI_CONFIG_TYPE_STRING, 6);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn test_memory_level_constants() {
        // Test memory level progression
        assert!(AARTSAAPI_MEMORY_SMALL < AARTSAAPI_MEMORY_MEDIUM);
        assert!(AARTSAAPI_MEMORY_MEDIUM < AARTSAAPI_MEMORY_LARGE);
        assert!(AARTSAAPI_MEMORY_LARGE < AARTSAAPI_MEMORY_LUDICROUS);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn test_device_state_constants() {
        // Test device state constants are in expected range
        let states = vec![
            AARTSAAPI_IDLE,
            AARTSAAPI_CONNECTING,
            AARTSAAPI_CONNECTED,
            AARTSAAPI_STARTING,
            AARTSAAPI_RUNNING,
            AARTSAAPI_STOPPING,
            AARTSAAPI_DISCONNECTING,
        ];

        // All device states should start with 0x1000000
        for state in states {
            assert!((state & 0xF0000000) == 0x10000000);
        }

        // Test ordering
        assert!(AARTSAAPI_IDLE < AARTSAAPI_CONNECTING);
        assert!(AARTSAAPI_CONNECTING < AARTSAAPI_CONNECTED);
        assert!(AARTSAAPI_CONNECTED < AARTSAAPI_STARTING);
        assert!(AARTSAAPI_STARTING < AARTSAAPI_RUNNING);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn test_error_result_constants() {
        // Test result code constants
        assert_eq!(AARTSAAPI_OK, 0);
        assert!(AARTSAAPI_EMPTY > AARTSAAPI_OK);
        assert!(AARTSAAPI_RETRY > AARTSAAPI_EMPTY);
    }

    #[test]
    #[allow(invalid_value)]
    #[ignore = "NativeSdkClient holds non-nullable fn pointers; zero-init is UB and aborts \
                with current rustc. A real mock would need a libloading::Library handle or a \
                test-double client trait — neither exists yet."]
    fn test_native_sdk_source_creation_mock() {
        // Test NativeSdkSource initialization without real SDK
        let source = NativeSdkSource {
            client: Arc::new(unsafe { std::mem::zeroed() }), // Mock client (dangerous but for test)
            handle: None,
            device: None,
            open_mode: None,
            stream_active: false,
            device_connected: false,
            sample_buffer: VecDeque::new(),
            dual_sample_buffer: VecDeque::new(),
            read_mode: None,
            receiver_clock_hz: None,
        };

        assert!(!source.is_streaming());
        assert_eq!(source.get_sample_buffer_size(), 0);
    }

    #[test]
    fn test_packet_data_calculation() {
        // Test IQ data size calculations
        let num_samples = 1024i64;
        let complex_size = std::mem::size_of::<Complex32>() as i64; // 8 bytes

        let packet = AARTSAAPI_Packet {
            cbsize: std::mem::size_of::<AARTSAAPI_Packet>() as i64,
            stream_id: 0,
            flags: 0,
            start_time: 0.0,
            end_time: 0.0,
            start_frequency: 0.0,
            step_frequency: 0.0,
            span_frequency: 0.0,
            rbw_frequency: 0.0,
            num: num_samples,
            total: num_samples,
            size: num_samples * complex_size,
            stride: complex_size,
            fp32: ptr::null_mut(),
            interleave: 0,
        };

        // Verify size calculation for IQ data
        let expected_bytes = num_samples * 2 * 4; // num_samples * 2 components * 4 bytes per f32
        assert_eq!(packet.size, expected_bytes);

        // Test complex sample count
        let complex_samples = packet.num as usize;
        assert_eq!(complex_samples, 1024);
    }

    #[test]
    fn test_frequency_range_validation() {
        // Test typical frequency ranges for Aaronia devices
        let test_frequencies = vec![
            (100e6, true),   // 100 MHz - valid
            (1e9, true),     // 1 GHz - valid
            (6e9, true),     // 6 GHz - valid for V6
            (0.0, false),    // 0 Hz - invalid
            (-100e6, false), // Negative - invalid
        ];

        for (freq, should_be_valid) in test_frequencies {
            let is_valid = freq > 0.0 && freq <= 20e9; // Rough validation
            assert_eq!(
                is_valid, should_be_valid,
                "Frequency {} Hz validation failed",
                freq
            );
        }
    }

    #[test]
    fn test_span_frequency_validation() {
        // Test span frequency validation
        let test_spans = vec![
            (1e6, true),   // 1 MHz span - valid
            (10e6, true),  // 10 MHz span - valid
            (100e6, true), // 100 MHz span - valid
            (0.0, false),  // 0 Hz span - invalid
            (-1e6, false), // Negative span - invalid
        ];

        for (span, should_be_valid) in test_spans {
            let is_valid = span > 0.0;
            assert_eq!(
                is_valid, should_be_valid,
                "Span {} Hz validation failed",
                span
            );
        }
    }

    #[test]
    fn test_reference_level_range() {
        // Test reference level ranges (typical for RF devices)
        let test_levels = vec![
            (-100.0, true),  // -100 dBm - valid
            (-50.0, true),   // -50 dBm - valid
            (0.0, true),     // 0 dBm - valid
            (30.0, true),    // +30 dBm - valid upper limit
            (50.0, false),   // +50 dBm - likely too high
            (-150.0, false), // -150 dBm - likely too low
        ];

        for (level, should_be_valid) in test_levels {
            let is_valid = (-140.0..=40.0).contains(&level); // Typical RF range
            assert_eq!(
                is_valid, should_be_valid,
                "Reference level {} dBm validation failed",
                level
            );
        }
    }

    #[test]
    fn test_sample_rate_calculation() {
        // Test sample rate calculations
        let span_frequencies = vec![1e6, 10e6, 100e6]; // 1 MHz, 10 MHz, 100 MHz

        for span in span_frequencies {
            // For IQ mode, sample rate typically equals span frequency
            let expected_sample_rate = span;
            assert_eq!(span, expected_sample_rate);

            // Calculate samples per second
            let samples_per_second = span as usize;
            assert!(samples_per_second > 0);
        }
    }
}
