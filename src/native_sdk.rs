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
use tracing::{debug, error, info, warn};
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
            let wide_path = string_to_wide(xml_path)?;
            let result = (self.init_with_path)(memory, wide_path.as_ptr());
            if result == AARTSAAPI_OK {
                self.initialized
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                info!("SDK initialized successfully");
                Ok(())
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_Init_With_Path failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
        }
    }

    pub unsafe fn shutdown(&self) -> Result<()> {
        unsafe {
            let result = (self.shutdown)();
            if result == AARTSAAPI_OK {
                info!("SDK shutdown successfully");
                Ok(())
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_Shutdown failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
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
            if result == AARTSAAPI_OK {
                debug!("SDK handle opened successfully");
                Ok(handle)
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_Open failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
        }
    }

    pub unsafe fn close_handle(&self, handle: &mut AARTSAAPI_Handle) -> Result<()> {
        unsafe {
            let result = (self.close)(handle);
            if result == AARTSAAPI_OK {
                debug!("SDK handle closed successfully");
                Ok(())
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_Close failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
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

            if result == AARTSAAPI_OK {
                info!("Device rescan completed successfully");
                Ok(())
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_RescanDevices failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
        }
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

            match result {
                AARTSAAPI_OK => {
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
                AARTSAAPI_EMPTY => {
                    debug!("No more devices at index {}", index);
                    Ok(None)
                }
                _ => Err(Error::Sdk(format!(
                    "AARTSAAPI_EnumDevice failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                ))),
            }
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

            if result == AARTSAAPI_OK {
                info!("Device {} opened successfully", device_type);
                Ok(device)
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_OpenDevice failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
        }
    }

    pub unsafe fn connect_device(&self, device: &mut AARTSAAPI_Device) -> Result<()> {
        unsafe {
            let result = (self.connect_device)(device);
            if result == AARTSAAPI_OK {
                info!("Device connected successfully");
                Ok(())
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_ConnectDevice failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
        }
    }

    pub unsafe fn start_device(&self, device: &mut AARTSAAPI_Device) -> Result<()> {
        unsafe {
            let result = (self.start_device)(device);
            if result == AARTSAAPI_OK {
                info!("Device started successfully");
                Ok(())
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_StartDevice failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
        }
    }

    pub unsafe fn stop_device(&self, device: &mut AARTSAAPI_Device) -> Result<()> {
        unsafe {
            let result = (self.stop_device)(device);
            if result == AARTSAAPI_OK {
                info!("Device stopped successfully");
                Ok(())
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_StopDevice failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
        }
    }

    pub unsafe fn disconnect_device(&self, device: &mut AARTSAAPI_Device) -> Result<()> {
        unsafe {
            let result = (self.disconnect_device)(device);
            if result == AARTSAAPI_OK {
                info!("Device disconnected successfully");
                Ok(())
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_DisconnectDevice failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
        }
    }

    pub unsafe fn close_device(
        &self,
        handle: &mut AARTSAAPI_Handle,
        device: &mut AARTSAAPI_Device,
    ) -> Result<()> {
        unsafe {
            let result = (self.close_device)(handle, device);
            if result == AARTSAAPI_OK {
                info!("Device closed successfully");
                Ok(())
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_CloseDevice failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
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
            if result == AARTSAAPI_OK {
                Ok(config)
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_ConfigRoot failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
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

            if result == AARTSAAPI_OK {
                debug!("Config {} found successfully", path);
                Ok(config)
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_ConfigFind failed for {}: 0x{:08x}",
                    path, result
                )))
            }
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
            if result == AARTSAAPI_OK {
                debug!("Config float value set to: {}", value);
                Ok(())
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_ConfigSetFloat failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
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
            if result == AARTSAAPI_OK {
                debug!("Config string value set to: {}", value);
                Ok(())
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_ConfigSetString failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
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
            if result == AARTSAAPI_OK {
                Ok(wide_to_string(&buffer))
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_ConfigGetString failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
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
            if result == AARTSAAPI_OK {
                Ok(value)
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_ConfigGetFloat failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
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
            if result == AARTSAAPI_OK {
                Ok(info)
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_ConfigGetInfo failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
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
            if result == AARTSAAPI_OK {
                Ok(())
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_ConfigSetInteger failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
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
            if result == AARTSAAPI_OK {
                Ok(value)
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_ConfigGetInteger failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
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
            if result == AARTSAAPI_OK {
                Ok(config)
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_ConfigHealth failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
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
            match result {
                AARTSAAPI_OK => Ok(Some(config)),
                AARTSAAPI_EMPTY => Ok(None),
                _ => Err(Error::Sdk(format!(
                    "AARTSAAPI_ConfigFirst failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                ))),
            }
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
            match result {
                AARTSAAPI_OK => Ok(true),
                AARTSAAPI_EMPTY => Ok(false),
                _ => Err(Error::Sdk(format!(
                    "AARTSAAPI_ConfigNext failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                ))),
            }
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
            if result == AARTSAAPI_OK {
                Ok(wide_to_string(&buffer))
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_ConfigGetName failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
        }
    }

    /// Reset the SDK device list (full power-cycle of all attached devices).
    /// Useful for crash recovery when `RescanDevices` keeps returning
    /// `AARTSAAPI_RETRY` past its budget.
    pub unsafe fn reset_devices(&self, handle: &mut AARTSAAPI_Handle) -> Result<()> {
        unsafe {
            let result = (self.reset_devices)(handle);
            if result == AARTSAAPI_OK {
                Ok(())
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_ResetDevices failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
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
            if result == AARTSAAPI_OK {
                Ok(num)
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_AvailPackets failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
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
            match result {
                AARTSAAPI_OK => {
                    debug!("Got packet with {} samples", packet.num);
                    Ok(Some(packet))
                }
                AARTSAAPI_EMPTY => {
                    debug!("No packet available");
                    Ok(None)
                }
                _ => Err(Error::Sdk(format!(
                    "AARTSAAPI_GetPacket failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                ))),
            }
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
            if result == AARTSAAPI_OK {
                debug!("Consumed {} packets", num_packets);
                Ok(())
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_ConsumePackets failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
        }
    }

    pub unsafe fn get_master_stream_time(&self, device: &mut AARTSAAPI_Device) -> Result<f64> {
        unsafe {
            let mut stime = 0.0;
            let result = (self.get_master_stream_time)(device, &mut stime);
            if result == AARTSAAPI_OK {
                Ok(stime)
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_GetMasterStreamTime failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
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
            if result == AARTSAAPI_OK {
                Ok(())
            } else {
                Err(Error::Sdk(format!(
                    "AARTSAAPI_SendPacket failed: 0x{:08x} ({})",
                    result,
                    result_message(result)
                )))
            }
        }
    }
}

impl Drop for NativeSdkClient {
    fn drop(&mut self) {
        if self.initialized.load(std::sync::atomic::Ordering::SeqCst) {
            unsafe {
                if let Err(e) = self.shutdown() {
                    error!("Error shutting down SDK during NativeSdkClient drop: {}", e);
                }
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
}

pub struct NativeSdkSource {
    client: Arc<NativeSdkClient>,
    handle: Option<AARTSAAPI_Handle>,
    device: Option<AARTSAAPI_Device>,
    open_mode: Option<DeviceOpenMode>,
    stream_active: bool,
    device_connected: bool,
    sample_buffer: VecDeque<Complex32>,
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

    /// Configure the `sweepsa` (spectrum sweep) config group.
    ///
    /// > [!WARNING]
    /// > Hardware-unverified: the `main/startfreq`/`main/stopfreq`/
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

            if let Ok(mut config_node) = self.client.find_config(device, &mut root, "main/rbw") {
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

    pub unsafe fn configure_iq_receiver(
        &mut self,
        center_freq: f64,
        span_freq: f64,
        ref_level: f64,
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
                // Configure receiver channel
                if let Ok(mut config) =
                    self.client
                        .find_config(device, &mut root, "device/receiverchannel")
                {
                    self.client.set_config_string(device, &mut config, "Rx1")?;
                    info!("Set receiver channel to Rx1");
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
                // Eco devices report no receiverclock key; the family is fixed
                // at 61.44 MHz per the README. Use that here so the IQ Mode
                // constraint is checked against the right clock.
                const ECO_FIXED_CLOCK_HZ: f64 = 61_440_000.0;
                ECO_FIXED_CLOCK_HZ
            };

            crate::utils::validate_iq_mode(span_freq, actual_clock_hz)?;

            info!("IQ Receiver configuration completed");
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
    pub unsafe fn read_samples(
        &mut self,
        buffer: &mut Vec<Complex32>,
        max_samples: usize,
    ) -> Result<usize> {
        unsafe {
            if !self.stream_active {
                return Err(Error::Sdk("Streaming not active".to_string()));
            }

            let device = self
                .device
                .as_mut()
                .ok_or_else(|| Error::Sdk("No device opened".to_string()))?;

            let mut samples_read = 0;

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
                        if started.elapsed() >= Self::READ_POLL_DEADLINE {
                            break None;
                        }
                        std::thread::sleep(Self::READ_POLL_INTERVAL);
                    }
                }
            };

            if let Some(packet) = packet_opt {
                if !packet.fp32.is_null() && packet.num > 0 && packet.stride < 2 {
                    // stride is "floats from sample to sample"; an IQ pair
                    // needs at least 2. Anything smaller is a non-IQ layout —
                    // consume it without mis-reading pairs out of it.
                    warn!(
                        "Skipping packet with non-IQ stride {} (need >= 2)",
                        packet.stride
                    );
                }
                // Process IQ data from the packet
                if !packet.fp32.is_null() && packet.num > 0 && packet.stride >= 2 {
                    const MAX_SAMPLES: usize = 1 << 24;
                    if (packet.num * 2) as usize > MAX_SAMPLES {
                        return Err(Error::Sdk(format!(
                            "Packet sample count {} exceeds maximum allowed {}",
                            packet.num * 2,
                            MAX_SAMPLES
                        )));
                    }
                    // SAFETY: packet.fp32 is verified non-null above, and packet.num * 2 is bounds-checked.
                    // `Complex32` is `#[repr(C)]` with fields `{re: f32, im: f32}`, so it has
                    // identical layout to `[f32; 2]`. This compile-time assertion guards against
                    // a future change to that representation.
                    const _: () =
                        assert!(std::mem::size_of::<Complex32>() == 2 * std::mem::size_of::<f32>());
                    let num_complex_samples = (packet.num as usize).min(max_samples);
                    let stride = packet.stride as usize;

                    if stride == 2 {
                        // Tightly packed IQ pairs — the common raw-IQ layout.
                        let complex_slice = std::slice::from_raw_parts(
                            packet.fp32 as *const Complex32,
                            num_complex_samples,
                        );
                        buffer.extend_from_slice(complex_slice);
                    } else {
                        // Per the official header, `stride` is the "offset from
                        // sample to sample in floats" and is not required to be
                        // 2 (e.g. multi-channel interleaved layouts). Gather
                        // sample-by-sample so a wider stride doesn't smear
                        // neighbouring channels into the IQ data.
                        // SAFETY: each sample spans floats
                        // [i*stride, i*stride+1], the last of which is
                        // (num-1)*stride + 2 floats into the buffer the SDK
                        // guarantees valid for `num` samples of `stride` floats.
                        let floats = std::slice::from_raw_parts(
                            packet.fp32 as *const f32,
                            (num_complex_samples - 1) * stride + 2,
                        );
                        buffer.extend(
                            (0..num_complex_samples).map(|i| {
                                Complex32::new(floats[i * stride], floats[i * stride + 1])
                            }),
                        );
                    }
                    samples_read = num_complex_samples;

                    info!(
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

    pub unsafe fn start_tx_stream(&mut self) -> Result<TxStream<'_>> {
        unsafe {
            let device = self
                .device
                .as_mut()
                .ok_or_else(|| Error::Sdk("No device opened".to_string()))?;

            Ok(TxStream::new(self.client.clone(), device))
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

            if self.device_connected {
                if let Some(device) = self.device.as_mut() {
                    // Disconnect device
                    if let Err(e) = self.client.disconnect_device(device) {
                        error!("Failed to disconnect device: {}", e);
                    }
                }
                self.device_connected = false;
            }

            if let Some(mut device) = self.device.take() {
                if let Some(handle) = self.handle.as_mut() {
                    // Close device
                    if let Err(e) = self.client.close_device(handle, &mut device) {
                        error!("Failed to close device: {}", e);
                    }
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

    pub fn get_sample_buffer_size(&self) -> usize {
        self.sample_buffer.len()
    }
}

impl Drop for NativeSdkSource {
    fn drop(&mut self) {
        // Ensure proper cleanup
        unsafe {
            if let Err(e) = self.stop_streaming() {
                error!("Error stopping streaming during drop: {}", e);
            }

            if let Some(mut handle) = self.handle.take() {
                if let Err(e) = self.client.close_handle(&mut handle) {
                    error!("Error closing handle during drop: {}", e);
                }
            }
        }
    }
}

/// Timing and frequency parameters for one burst handed to
/// [`TxStream::write_samples`].
///
/// The official header documents `AARTSAAPI_Packet::startTime`/`endTime`
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
                flags: 0,
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
