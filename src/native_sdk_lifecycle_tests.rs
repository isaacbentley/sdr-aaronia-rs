//! SDK lifecycle fault injection. No Aaronia installation or USB device.
use super::*;
use std::cell::RefCell;
thread_local! {
    static CALLS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    static FAIL: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}
fn call(name: &'static str) -> u32 {
    CALLS.with(|calls| calls.borrow_mut().push(name));
    if FAIL.with(|fail| fail.borrow().contains(&name)) {
        AARTSAAPI_ERROR_BUSY
    } else {
        AARTSAAPI_OK
    }
}
fn reset(fail: &[&'static str]) {
    CALLS.with(|calls| calls.borrow_mut().clear());
    FAIL.with(|failed| *failed.borrow_mut() = fail.to_vec());
}
fn calls() -> Vec<&'static str> {
    CALLS.with(|calls| calls.borrow().clone())
}
unsafe extern "C" fn mock_init(_memory: u32) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_init_with_path(_memory: u32, _path: *const WideChar) -> u32 {
    call("init_with_path")
}
unsafe extern "C" fn mock_shutdown() -> u32 {
    call("shutdown")
}
unsafe extern "C" fn mock_version() -> u32 {
    0x01020304
}
unsafe extern "C" fn mock_open(_handle: *mut AARTSAAPI_Handle) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_close(_handle: *mut AARTSAAPI_Handle) -> u32 {
    call("close")
}
unsafe extern "C" fn mock_rescan_devices(_handle: *mut AARTSAAPI_Handle, _timeout: i32) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_reset_devices(_handle: *mut AARTSAAPI_Handle) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_enum_device(
    _handle: *mut AARTSAAPI_Handle,
    _device_type: *const WideChar,
    _index: i32,
    _info: *mut AARTSAAPI_DeviceInfo,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_open_device(
    _handle: *mut AARTSAAPI_Handle,
    _device: *mut AARTSAAPI_Device,
    _device_type: *const WideChar,
    _serial: *const WideChar,
) -> u32 {
    unsafe {
        (*_device).d = std::ptr::dangling_mut::<u8>().cast();
    }
    call("open_device")
}
unsafe extern "C" fn mock_close_device(
    _handle: *mut AARTSAAPI_Handle,
    _device: *mut AARTSAAPI_Device,
) -> u32 {
    call("close_device")
}
unsafe extern "C" fn mock_connect_device(_device: *mut AARTSAAPI_Device) -> u32 {
    call("connect_device")
}
unsafe extern "C" fn mock_disconnect_device(_device: *mut AARTSAAPI_Device) -> u32 {
    call("disconnect_device")
}
unsafe extern "C" fn mock_start_device(_device: *mut AARTSAAPI_Device) -> u32 {
    call("start_device")
}
unsafe extern "C" fn mock_stop_device(_device: *mut AARTSAAPI_Device) -> u32 {
    call("stop_device")
}
unsafe extern "C" fn mock_get_device_state(_device: *mut AARTSAAPI_Device) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_config_root(
    _device: *mut AARTSAAPI_Device,
    _config: *mut AARTSAAPI_Config,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_config_health(
    _device: *mut AARTSAAPI_Device,
    _config: *mut AARTSAAPI_Config,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_config_first(
    _device: *mut AARTSAAPI_Device,
    _group: *mut AARTSAAPI_Config,
    _config: *mut AARTSAAPI_Config,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_config_next(
    _device: *mut AARTSAAPI_Device,
    _group: *mut AARTSAAPI_Config,
    _config: *mut AARTSAAPI_Config,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_config_get_name(
    _device: *mut AARTSAAPI_Device,
    _config: *mut AARTSAAPI_Config,
    _name: *mut WideChar,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_config_find(
    _device: *mut AARTSAAPI_Device,
    _group: *mut AARTSAAPI_Config,
    _config: *mut AARTSAAPI_Config,
    _path: *const WideChar,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_config_set_float(
    _device: *mut AARTSAAPI_Device,
    _config: *mut AARTSAAPI_Config,
    _value: f64,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_config_set_string(
    _device: *mut AARTSAAPI_Device,
    _config: *mut AARTSAAPI_Config,
    _value: *const WideChar,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_config_set_integer(
    _device: *mut AARTSAAPI_Device,
    _config: *mut AARTSAAPI_Config,
    _value: i64,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_config_get_string(
    _device: *mut AARTSAAPI_Device,
    _config: *mut AARTSAAPI_Config,
    _value: *mut WideChar,
    _size: *mut i64,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_config_get_info(
    _device: *mut AARTSAAPI_Device,
    _config: *mut AARTSAAPI_Config,
    _info: *mut AARTSAAPI_ConfigInfo,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_config_get_float(
    _device: *mut AARTSAAPI_Device,
    _config: *mut AARTSAAPI_Config,
    _value: *mut f64,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_config_get_integer(
    _device: *mut AARTSAAPI_Device,
    _config: *mut AARTSAAPI_Config,
    _value: *mut i64,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_avail_packets(
    _device: *mut AARTSAAPI_Device,
    _channel: i32,
    _num: *mut i32,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_get_packet(
    _device: *mut AARTSAAPI_Device,
    _channel: i32,
    _index: i32,
    _packet: *mut AARTSAAPI_Packet,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_consume_packets(
    _device: *mut AARTSAAPI_Device,
    _channel: i32,
    _num: i32,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_get_master_stream_time(
    _device: *mut AARTSAAPI_Device,
    _stime: *mut f64,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
unsafe extern "C" fn mock_send_packet(
    _device: *mut AARTSAAPI_Device,
    _channel: i32,
    _packet: *const AARTSAAPI_Packet,
) -> u32 {
    AARTSAAPI_ERROR_NOT_FOUND
}
fn client() -> Arc<NativeSdkClient> {
    #[cfg(target_os = "linux")]
    let lib = libloading::os::unix::Library::this().into();
    #[cfg(target_os = "windows")]
    let lib = libloading::os::windows::Library::this().unwrap().into();
    Arc::new(NativeSdkClient {
        _lib: lib,
        initialized: std::sync::atomic::AtomicBool::new(false),
        init: mock_init,
        init_with_path: mock_init_with_path,
        shutdown: mock_shutdown,
        version: mock_version,
        open: mock_open,
        close: mock_close,
        rescan_devices: mock_rescan_devices,
        reset_devices: mock_reset_devices,
        enum_device: mock_enum_device,
        open_device: mock_open_device,
        close_device: mock_close_device,
        connect_device: mock_connect_device,
        disconnect_device: mock_disconnect_device,
        start_device: mock_start_device,
        stop_device: mock_stop_device,
        get_device_state: mock_get_device_state,
        config_root: mock_config_root,
        config_health: mock_config_health,
        config_first: mock_config_first,
        config_next: mock_config_next,
        config_get_name: mock_config_get_name,
        config_find: mock_config_find,
        config_set_float: mock_config_set_float,
        config_set_string: mock_config_set_string,
        config_set_integer: mock_config_set_integer,
        config_get_string: mock_config_get_string,
        config_get_info: mock_config_get_info,
        config_get_float: mock_config_get_float,
        config_get_integer: mock_config_get_integer,
        avail_packets: mock_avail_packets,
        get_packet: mock_get_packet,
        consume_packets: mock_consume_packets,
        get_master_stream_time: mock_get_master_stream_time,
        send_packet: mock_send_packet,
    })
}
fn source(active: bool, connected: bool) -> NativeSdkSource {
    let mut source = NativeSdkSource::with_client(client());
    source.handle = Some(AARTSAAPI_Handle {
        d: std::ptr::dangling_mut::<u8>().cast(),
    });
    source.device = Some(AARTSAAPI_Device {
        d: std::ptr::dangling_mut::<u8>().cast(),
    });
    source.open_mode = Some(DeviceOpenMode::EcoIqReceiver);
    source.stream_active = active;
    source.device_connected = connected;
    source
}
#[test]
fn stop_attempts_every_cleanup_stage_and_returns_first_failure() {
    reset(&["stop_device", "disconnect_device"]);
    let mut source = source(true, true);
    assert!(
        matches!(unsafe { source.stop_streaming() }, Err(Error::SdkApi { operation, .. }) if operation == "AARTSAAPI_StopDevice")
    );
    assert_eq!(
        calls(),
        vec!["stop_device", "disconnect_device", "close_device"]
    );
    assert!(!source.is_device_open());
    assert!(!source.is_streaming());
    unsafe {
        source.stop_streaming().unwrap();
    }
    assert_eq!(calls().len(), 3, "idempotent stop must not close twice");
}
#[test]
fn failed_start_releases_partial_connection() {
    reset(&["start_device"]);
    let mut source = source(false, false);
    assert!(unsafe { source.start_streaming() }.is_err());
    assert!(!source.is_device_open());
    assert_eq!(
        calls(),
        vec![
            "connect_device",
            "start_device",
            "stop_device",
            "disconnect_device",
            "close_device"
        ]
    );
}
#[test]
fn failed_connect_releases_partial_connection() {
    reset(&["connect_device"]);
    let mut source = source(false, false);
    assert!(unsafe { source.start_streaming() }.is_err());
    assert_eq!(
        calls(),
        vec!["connect_device", "disconnect_device", "close_device"]
    );
}
#[test]
fn drop_closes_an_open_device_that_never_started() {
    reset(&[]);
    drop(source(false, false));
    assert_eq!(calls(), vec!["close_device", "close"]);
}
#[test]
fn failed_close_retains_handle_for_drop_cleanup() {
    reset(&["close_device"]);
    let mut source = source(true, true);
    assert!(unsafe { source.stop_streaming() }.is_err());
    assert!(source.is_device_open());
    FAIL.with(|failed| failed.borrow_mut().clear());
    drop(source);
    assert_eq!(
        calls(),
        vec![
            "stop_device",
            "disconnect_device",
            "close_device",
            "close_device",
            "close"
        ]
    );
}
#[test]
fn failed_device_open_closes_a_partially_allocated_handle() {
    reset(&["open_device"]);
    let client = client();
    let mut handle = AARTSAAPI_Handle {
        d: std::ptr::dangling_mut::<u8>().cast(),
    };
    let serial = string_to_wide("fixture").unwrap();
    assert!(
        unsafe {
            client.open_device(
                &mut handle,
                "spectranv6eco/iqreceiver",
                serial.as_slice_with_nul(),
            )
        }
        .is_err()
    );
    assert_eq!(calls(), vec!["open_device", "close_device"]);
}
#[test]
fn sdk_shutdown_waits_for_the_last_initialized_client() {
    reset(&[]);
    let first = client();
    let second = client();
    unsafe {
        first.init_with_path(1, "fixture").unwrap();
        second.init_with_path(1, "fixture").unwrap();
        first.shutdown().unwrap();
        assert_eq!(calls(), vec!["init_with_path"]);
        second.shutdown().unwrap();
        second.shutdown().unwrap();
    }
    assert_eq!(calls(), vec!["init_with_path", "shutdown"]);
}
