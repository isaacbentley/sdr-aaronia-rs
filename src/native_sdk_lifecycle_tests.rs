//! SDK lifecycle fault injection. No Aaronia installation or USB device.
use super::*;
use std::cell::{Cell, RefCell};

static LIFECYCLE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One queued IQ packet for the fake `GetPacket`/`ConsumePackets`.
struct FakePacket {
    start_frequency: f64,
    step_frequency: f64,
    span_frequency: f64,
    /// Floats per sample: 2 for mono IQ, 4 for the dual interleave.
    stride: i64,
    floats: Vec<f32>,
    /// `GetPacket` calls answered `EMPTY` before this packet shows: a
    /// packet that arrives while a read is already polling.
    empty_polls_before: u32,
}

impl FakePacket {
    /// `samples` IQ samples of `value`, `stride` floats apart, under the
    /// geometry of a `span` centred on `center`.
    fn iq(center: f64, span: f64, stride: i64, samples: usize, value: f32) -> Self {
        Self {
            start_frequency: center - span / 2.0,
            step_frequency: span,
            span_frequency: span,
            stride,
            floats: vec![value; samples * stride as usize],
            empty_polls_before: 0,
        }
    }
}

thread_local! {
    static CALLS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    static FAIL: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    static CONFIG_AVAILABLE: Cell<bool> = const { Cell::new(false) };
    static LAST_OPEN_SERIAL: RefCell<Option<String>> = const { RefCell::new(None) };
    static PACKETS: RefCell<std::collections::VecDeque<FakePacket>> =
        const { RefCell::new(std::collections::VecDeque::new()) };
}
fn queue(packet: FakePacket) {
    PACKETS.with(|packets| packets.borrow_mut().push_back(packet));
}
fn queued() -> usize {
    PACKETS.with(|packets| packets.borrow().len())
}
fn call(name: &'static str) -> u32 {
    CALLS.with(|calls| calls.borrow_mut().push(name));
    if FAIL.with(|fail| fail.borrow().contains(&name)) {
        AARTSAAPI_ERROR_BUSY
    } else {
        AARTSAAPI_OK
    }
}
fn reset(fail: &[&'static str]) -> std::sync::MutexGuard<'static, ()> {
    let guard = LIFECYCLE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    *SDK_LIVE_INITS.lock().unwrap_or_else(|p| p.into_inner()) = 0;
    CONFIG_AVAILABLE.with(|available| available.set(false));
    LAST_OPEN_SERIAL.with(|serial| *serial.borrow_mut() = None);
    CALLS.with(|calls| calls.borrow_mut().clear());
    FAIL.with(|failed| *failed.borrow_mut() = fail.to_vec());
    PACKETS.with(|packets| packets.borrow_mut().clear());
    guard
}
fn calls() -> Vec<&'static str> {
    CALLS.with(|calls| calls.borrow().clone())
}
fn last_open_serial() -> Option<String> {
    LAST_OPEN_SERIAL.with(|serial| serial.borrow().clone())
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
    assert!(!_serial.is_null(), "serial pointer must never be null");
    assert_ne!(
        _serial as usize,
        std::mem::align_of::<WideChar>(),
        "serial pointer must never be a dangling empty-slice pointer"
    );
    unsafe {
        let mut len = 0usize;
        while *_serial.add(len) != 0 {
            len += 1;
            assert!(len <= 256, "serial pointer must be NUL-terminated");
        }
        let slice = std::slice::from_raw_parts(_serial, len);
        LAST_OPEN_SERIAL.with(|s| *s.borrow_mut() = Some(wide_to_string(slice)));
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
    if CONFIG_AVAILABLE.with(Cell::get) {
        unsafe {
            (*_config).d = std::ptr::dangling_mut::<u8>().cast();
        }
        AARTSAAPI_OK
    } else {
        AARTSAAPI_ERROR_NOT_FOUND
    }
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
    if CONFIG_AVAILABLE.with(Cell::get) {
        unsafe {
            (*_config).d = std::ptr::dangling_mut::<u8>().cast();
        }
        AARTSAAPI_OK
    } else {
        AARTSAAPI_ERROR_NOT_FOUND
    }
}
unsafe extern "C" fn mock_config_set_float(
    _device: *mut AARTSAAPI_Device,
    _config: *mut AARTSAAPI_Config,
    _value: f64,
) -> u32 {
    call("config_write")
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
    if !CONFIG_AVAILABLE.with(Cell::get) {
        return AARTSAAPI_ERROR_NOT_FOUND;
    }
    unsafe {
        (*_info).min_value = -55.0;
        (*_info).max_value = 23.0;
        (*_info).step_value = 0.5;
    }
    AARTSAAPI_OK
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
    packet: *mut AARTSAAPI_Packet,
) -> u32 {
    PACKETS.with(|packets| {
        let mut packets = packets.borrow_mut();
        let Some(head) = packets.front_mut() else {
            return AARTSAAPI_EMPTY;
        };
        if head.empty_polls_before > 0 {
            head.empty_polls_before -= 1;
            return AARTSAAPI_EMPTY;
        }
        // The floats stay in the queue, unmoved, until consumed: the
        // same lifetime the SDK gives a packet's buffer.
        unsafe {
            (*packet).start_frequency = head.start_frequency;
            (*packet).step_frequency = head.step_frequency;
            (*packet).span_frequency = head.span_frequency;
            (*packet).num = head.floats.len() as i64 / head.stride;
            (*packet).stride = head.stride;
            (*packet).size = head.stride;
            (*packet).fp32 = head.floats.as_mut_ptr();
        }
        AARTSAAPI_OK
    })
}
unsafe extern "C" fn mock_consume_packets(
    _device: *mut AARTSAAPI_Device,
    _channel: i32,
    num: i32,
) -> u32 {
    PACKETS.with(|packets| {
        let mut packets = packets.borrow_mut();
        for _ in 0..num {
            packets.pop_front();
        }
    });
    AARTSAAPI_OK
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
        _lib: std::mem::ManuallyDrop::new(lib),
        initialized: std::sync::atomic::AtomicBool::new(false),
        active_handles: std::sync::atomic::AtomicUsize::new(0),
        leaked_resource: std::sync::atomic::AtomicBool::new(false),
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
    let c = client();
    c.active_handles
        .store(2, std::sync::atomic::Ordering::SeqCst);
    let mut source = NativeSdkSource::with_client(c);
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
    let _guard = reset(&["stop_device", "disconnect_device"]);
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
    let _guard = reset(&["start_device"]);
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
    let _guard = reset(&["connect_device"]);
    let mut source = source(false, false);
    assert!(unsafe { source.start_streaming() }.is_err());
    assert_eq!(
        calls(),
        vec!["connect_device", "disconnect_device", "close_device"]
    );
}
#[test]
fn drop_closes_an_open_device_that_never_started() {
    let _guard = reset(&[]);
    drop(source(false, false));
    assert_eq!(calls(), vec!["close_device", "close"]);
}
#[test]
fn failed_close_retains_handle_for_drop_cleanup() {
    let _guard = reset(&["close_device"]);
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
    let _guard = reset(&["open_device"]);
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
    let _guard = reset(&[]);
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

#[test]
fn failed_shutdown_preserves_live_init_count_and_skips_unload() {
    let _guard = reset(&["shutdown"]);
    let c = client();
    unsafe {
        c.init_with_path(1, "fixture").unwrap();
        assert!(c.shutdown().is_err());
        assert!(c.initialized.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(*SDK_LIVE_INITS.lock().unwrap(), 1);
        FAIL.with(|failed| failed.borrow_mut().clear());
        c.shutdown().unwrap();
        assert!(!c.initialized.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(*SDK_LIVE_INITS.lock().unwrap(), 0);
    }
}

#[test]
fn open_device_accepts_empty_and_unterminated_serial_slices() {
    let _guard = reset(&[]);
    let client = client();
    let mut handle = AARTSAAPI_Handle {
        d: std::ptr::dangling_mut::<u8>().cast(),
    };
    unsafe {
        let mut dev1 = client
            .open_device(&mut handle, "spectranv6eco/iqreceiver", &[])
            .unwrap();
        assert_eq!(last_open_serial().as_deref(), Some(""));
        client.close_device(&mut handle, &mut dev1).unwrap();

        let unterminated = [b'A' as WideChar, b'B' as WideChar];
        let mut dev2 = client
            .open_device(&mut handle, "spectranv6eco/iqreceiver", &unterminated)
            .unwrap();
        assert_eq!(last_open_serial().as_deref(), Some("AB"));
        client.close_device(&mut handle, &mut dev2).unwrap();
    }
}

#[test]
fn sweep_reference_level_is_checked_before_any_write() {
    let _guard = reset(&[]);
    for level in [f64::NAN, f64::INFINITY, -56.0, 24.0] {
        CONFIG_AVAILABLE.with(|available| available.set(true));
        CALLS.with(|calls| calls.borrow_mut().clear());
        let mut source = source(false, false);
        assert!(
            unsafe {
                source.configure_sweepsa_with_reference_level(&Default::default(), Some(level))
            }
            .is_err()
        );
        assert!(!calls().contains(&"config_write"));
    }
    CONFIG_AVAILABLE.with(|available| available.set(true));
    CALLS.with(|calls| calls.borrow_mut().clear());
    let mut source = source(false, false);
    unsafe {
        source
            .configure_sweepsa_with_reference_level(&Default::default(), Some(-50.0))
            .unwrap();
    }
    assert_eq!(
        calls()
            .iter()
            .filter(|&&call| call == "config_write")
            .count(),
        5
    );
}
#[test]
fn missing_explicit_sweep_reference_metadata_fails_before_writes() {
    let _guard = reset(&[]);
    let mut source = source(false, false);
    assert!(
        unsafe { source.configure_sweepsa_with_reference_level(&Default::default(), Some(-50.0)) }
            .is_err()
    );
    assert!(!calls().contains(&"config_write"));
}

const DEADLINE: std::time::Duration = std::time::Duration::from_millis(500);

/// A deadline read keeps polling for more after the queue drains. A
/// packet of a new geometry that arrives during that poll — the first
/// packet after a retune — must end the read, not join it under the new
/// centre: the label would then describe only part of the buffer.
#[test]
fn a_deadline_read_stops_at_a_retune_that_arrives_mid_poll() {
    let _guard = reset(&[]);
    let mut source = source(true, true);
    queue(FakePacket::iq(915e6, 15.36e6, 2, 4, 1.0));
    queue(FakePacket {
        empty_polls_before: 1,
        ..FakePacket::iq(2_400e6, 15.36e6, 2, 4, 2.0)
    });

    let mut buffer = Vec::new();
    let deadline = std::time::Instant::now() + DEADLINE;
    let n = unsafe { source.read_samples_until(&mut buffer, 8, deadline) }.unwrap();
    assert_eq!(n, 4, "the read must stop before the retuned packet");
    assert!(buffer.iter().all(|s| s.re == 1.0));
    assert_eq!(source.observed_center_frequency_hz(), Some(915e6));
    assert_eq!(
        queued(),
        1,
        "the retuned packet stays queued for the next read"
    );

    buffer.clear();
    let deadline = std::time::Instant::now() + DEADLINE;
    let n = unsafe { source.read_samples_until(&mut buffer, 8, deadline) }.unwrap();
    assert_eq!(n, 4);
    assert!(buffer.iter().all(|s| s.re == 2.0));
    assert_eq!(source.observed_center_frequency_hz(), Some(2_400e6));
}

/// A carried tail and the packet after it are separate geometries when a
/// retune falls between them; the read drains the tail and stops.
#[test]
fn a_deadline_read_does_not_join_a_carried_tail_to_a_retuned_packet() {
    let _guard = reset(&[]);
    let mut source = source(true, true);
    queue(FakePacket::iq(915e6, 15.36e6, 2, 8, 1.0));

    let mut buffer = Vec::new();
    let deadline = std::time::Instant::now() + DEADLINE;
    let n = unsafe { source.read_samples_until(&mut buffer, 4, deadline) }.unwrap();
    assert_eq!(n, 4, "half the packet is carried");

    queue(FakePacket::iq(2_400e6, 15.36e6, 2, 4, 2.0));
    buffer.clear();
    let deadline = std::time::Instant::now() + DEADLINE;
    let n = unsafe { source.read_samples_until(&mut buffer, 8, deadline) }.unwrap();
    assert_eq!(n, 4, "only the carried tail belongs to this read");
    assert!(buffer.iter().all(|s| s.re == 1.0));
    assert_eq!(source.observed_center_frequency_hz(), Some(915e6));

    buffer.clear();
    let deadline = std::time::Instant::now() + DEADLINE;
    let n = unsafe { source.read_samples_until(&mut buffer, 8, deadline) }.unwrap();
    assert_eq!(n, 4);
    assert!(buffer.iter().all(|s| s.re == 2.0));
    assert_eq!(source.observed_center_frequency_hz(), Some(2_400e6));
}

/// Packets of one geometry still fill a read across polls.
#[test]
fn a_deadline_read_keeps_filling_from_one_geometry() {
    let _guard = reset(&[]);
    let mut source = source(true, true);
    queue(FakePacket::iq(915e6, 15.36e6, 2, 4, 1.0));
    queue(FakePacket {
        empty_polls_before: 1,
        ..FakePacket::iq(915e6, 15.36e6, 2, 4, 1.0)
    });
    let mut buffer = Vec::new();
    let deadline = std::time::Instant::now() + DEADLINE;
    let n = unsafe { source.read_samples_until(&mut buffer, 8, deadline) }.unwrap();
    assert_eq!(n, 8);
    assert_eq!(queued(), 0);
}

/// The dual path holds the same line, for its carried pairs and for a
/// packet that arrives mid-poll.
#[test]
fn a_dual_deadline_read_stops_at_a_retune() {
    let _guard = reset(&[]);
    let mut source = source(true, true);
    queue(FakePacket::iq(915e6, 15.36e6, 4, 8, 1.0));

    let (mut rx1, mut rx2) = (Vec::new(), Vec::new());
    let deadline = std::time::Instant::now() + DEADLINE;
    let n = unsafe { source.read_samples_dual_until(&mut rx1, &mut rx2, 4, deadline) }.unwrap();
    assert_eq!(n, 4, "half the packet is carried");

    queue(FakePacket {
        empty_polls_before: 1,
        ..FakePacket::iq(2_400e6, 15.36e6, 4, 4, 2.0)
    });
    rx1.clear();
    rx2.clear();
    let deadline = std::time::Instant::now() + DEADLINE;
    let n = unsafe { source.read_samples_dual_until(&mut rx1, &mut rx2, 8, deadline) }.unwrap();
    assert_eq!(n, 4, "only the carried pairs belong to this read");
    assert_eq!((rx1.len(), rx2.len()), (4, 4));
    assert!(rx1.iter().chain(&rx2).all(|s| s.re == 1.0));
    assert_eq!(source.observed_center_frequency_hz(), Some(915e6));

    rx1.clear();
    rx2.clear();
    let deadline = std::time::Instant::now() + DEADLINE;
    let n = unsafe { source.read_samples_dual_until(&mut rx1, &mut rx2, 8, deadline) }.unwrap();
    assert_eq!(n, 4);
    assert!(rx1.iter().chain(&rx2).all(|s| s.re == 2.0));
    assert_eq!(source.observed_center_frequency_hz(), Some(2_400e6));
}

/// Nothing within the deadline is a timeout, as the unified read promises.
#[test]
fn a_deadline_read_with_nothing_queued_times_out() {
    let _guard = reset(&[]);
    let mut source = source(true, true);
    let mut buffer = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(20);
    match unsafe { source.read_samples_until(&mut buffer, 8, deadline) } {
        Err(Error::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::TimedOut),
        other => panic!("expected a timeout, got {other:?}"),
    }
}
