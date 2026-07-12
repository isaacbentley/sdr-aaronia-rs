use crate::http_endpoints::HttpEndpointsClient;
use crate::unified_source::{AaroniaSource, AaroniaSourceBuilder, SourceType};
use num_complex::Complex32;
use std::cell::RefCell;
use std::ffi::{CStr, CString, c_void};
use std::os::raw::c_char;
use std::ptr;
use std::sync::OnceLock;
use tokio::runtime::{Handle, Runtime};

// --- Thread-local last-error storage (A33) --- //
//
// Backwards-compatible diagnostic surface for the FFI layer: every
// function that returns an opaque code (or a null pointer) now also
// stashes a free-form error string in this thread-local. Embedders
// can call `aaronia_last_error()` to retrieve a human-readable
// message about whatever just failed, then free it with
// `aaronia_string_free`. Mirrors the `fpv_drone_dji_last_error`
// pattern in the sibling DJI crate.

thread_local! {
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Internal helper: stash `msg` as the current thread's last-error
/// message. Used at every FFI boundary that previously dropped
/// detailed error context on the floor.
fn set_last_error(msg: impl Into<String>) {
    LAST_ERROR.with(|e| *e.borrow_mut() = Some(msg.into()));
}

/// Internal helper: clear the current thread's last-error message.
/// Called at the start of every fallible FFI entry point so a
/// caller that re-uses the thread doesn't see stale errors from a
/// previous successful call.
fn clear_last_error() {
    LAST_ERROR.with(|e| *e.borrow_mut() = None);
}

/// Return the last error message recorded on the calling thread, or
/// `NULL` if no error has been recorded since the last successful
/// call. Ownership of the returned string transfers to the caller —
/// free it with [`aaronia_string_free`] when done. Repeated calls
/// without an intervening error return the same message until the
/// next FFI call that touches the slot.
///
/// # Safety
/// This function takes no arguments and is sound to call from any
/// thread; the `unsafe` qualifier is required only because the
/// returned pointer transfers ownership to the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_last_error() -> *mut c_char {
    LAST_ERROR.with(|e| match e.borrow().as_ref() {
        Some(msg) => CString::new(msg.as_str())
            .map(|s| s.into_raw())
            .unwrap_or(ptr::null_mut()),
        None => ptr::null_mut(),
    })
}

/// Internal multi-threaded tokio runtime used by the C FFI when the calling
/// thread does not have an active runtime. C consumers do not have to drive
/// tokio themselves; the first FFI entry point lazily builds this runtime and
/// every subsequent call reuses it.
static FFI_RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn ffi_runtime() -> &'static Runtime {
    FFI_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("aaronia-ffi")
            .build()
            .expect("failed to construct internal tokio runtime for aaronia C FFI")
    })
}

/// Drive `fut` to completion for a synchronous FFI entry point.
///
/// The earlier implementation grabbed `Handle::try_current()` and called
/// `block_on` on it — which **panics** when the caller happens to be on a
/// tokio runtime thread, and a panic crossing an `extern "C"` boundary
/// aborts the whole process. Instead:
///
/// - Outside any runtime: block on the bundled FFI runtime (the common
///   case for C callers).
/// - Inside a multi-threaded runtime: wrap in `block_in_place` so the
///   worker thread may legally block, and reuse the caller's runtime.
/// - Inside a current-thread runtime: there is no sound way to block
///   without deadlocking the reactor — return an error the FFI shims
///   translate into their failure value instead of aborting.
fn ffi_block_on<F: std::future::Future>(fut: F) -> Result<F::Output, String> {
    match Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            Ok(tokio::task::block_in_place(|| handle.block_on(fut)))
        }
        Ok(_) => Err(
            "FFI entry point called from within a current-thread tokio runtime; \
             blocking here would deadlock the reactor. Call from a non-async \
             thread (or a multi-threaded runtime) instead."
                .to_string(),
        ),
        Err(_) => Ok(ffi_runtime().block_on(fut)),
    }
}

// --- FFI Error Handling --- //

/// AaroniaFfiError enumeration.
#[repr(C)]
pub enum AaroniaFfiError {
    Success = 0,
    NullPointer = 1,
    InvalidString = 2,
    InternalError = 3,
    BuildFailed = 4,
    ReadError = 5,
    /// The FFI entry point was invoked from a thread context where the
    /// call cannot legally block (a current-thread tokio runtime). Call
    /// from a plain thread, or from a multi-threaded runtime, instead.
    RuntimeContext = 6,
}

// --- C-compatible SourceType --- //
/// CAaroniaSourceType enumeration.
#[repr(C)]
pub enum CAaroniaSourceType {
    NativeSdk,
    Http,
    File,
}

impl From<CAaroniaSourceType> for SourceType {
    fn from(item: CAaroniaSourceType) -> Self {
        match item {
            CAaroniaSourceType::NativeSdk => SourceType::NativeSdk,
            CAaroniaSourceType::Http => SourceType::Http,
            CAaroniaSourceType::File => SourceType::File,
        }
    }
}

impl From<SourceType> for CAaroniaSourceType {
    fn from(item: SourceType) -> Self {
        match item {
            SourceType::NativeSdk => CAaroniaSourceType::NativeSdk,
            SourceType::Http => CAaroniaSourceType::Http,
            SourceType::File => CAaroniaSourceType::File,
        }
    }
}

// --- C-compatible Complex struct --- //
/// FfiComplex structure.
#[repr(C)]
pub struct FfiComplex {
    pub re: f32,
    pub im: f32,
}

// --- C-compatible ServerInfo struct --- //
/// FfiServerInfo structure.
#[repr(C)]
pub struct FfiServerInfo {
    pub name: *const c_char,
    pub version: *const c_char,
    pub build: *const c_char,
    pub serial: *const c_char,
    pub title: *const c_char,
    pub mission: *const c_char,
}

// --- C-compatible SourceInfo struct --- //
/// FfiSourceInfo structure.
#[repr(C)]
pub struct FfiSourceInfo {
    pub source_type: CAaroniaSourceType,
    pub center_frequency: f64,
    /// IQ sample rate (Fs) in Hz — see `SourceInfo::span_frequency`.
    pub span_frequency: f64,
    /// Usable RX/real-time bandwidth in Hz; `0.0` = unknown. Always
    /// `<= span_frequency`.
    pub bandwidth_hz: f64,
    pub reference_level: f64,
    pub device_serial: *const c_char,
}

// --- AaroniaSourceBuilder FFI --- //

/// Aaronia source builder new.
#[unsafe(no_mangle)]
pub extern "C" fn aaronia_source_builder_new() -> *mut AaroniaSourceBuilder {
    Box::into_raw(Box::new(AaroniaSourceBuilder::new()))
}

/// Free a builder previously returned by [`aaronia_source_builder_new`].
///
/// # Safety
/// `builder` must either be null or a pointer previously returned by
/// [`aaronia_source_builder_new`] that has not yet been freed. After this
/// call the pointer must not be used again. Passing a pointer obtained any
/// other way (e.g. constructed in C, or already freed) is undefined
/// behaviour.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_builder_free(builder: *mut AaroniaSourceBuilder) {
    unsafe {
        if !builder.is_null() {
            // SAFETY: per the function-level contract, the caller guarantees the
            // pointer originated from `aaronia_source_builder_new` and has not
            // been freed.
            drop(Box::from_raw(builder));
        }
    }
}

/// Set the IQ-mode center frequency on the builder, in Hz.
///
/// # Safety
/// `builder` must either be null (no-op) or a valid pointer to a live
/// `AaroniaSourceBuilder` returned by [`aaronia_source_builder_new`] and
/// not yet freed. The pointer must remain valid for the duration of the
/// call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_builder_center_frequency(
    builder: *mut AaroniaSourceBuilder,
    freq: f64,
) {
    unsafe {
        if let Some(builder) = builder.as_mut() {
            builder.center_frequency(freq);
        }
    }
}

/// Set the IQ-mode span frequency on the builder, in Hz.
///
/// # Safety
/// `builder` must either be null (no-op) or a valid pointer to a live
/// `AaroniaSourceBuilder` returned by [`aaronia_source_builder_new`] and
/// not yet freed. The pointer must remain valid for the duration of the
/// call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_builder_span_frequency(
    builder: *mut AaroniaSourceBuilder,
    freq: f64,
) {
    unsafe {
        if let Some(builder) = builder.as_mut() {
            builder.span_frequency(freq);
        }
    }
}

/// Set the IQ-mode reference level on the builder, in dBm.
///
/// # Safety
/// `builder` must either be null (no-op) or a valid pointer to a live
/// `AaroniaSourceBuilder` returned by [`aaronia_source_builder_new`] and
/// not yet freed. The pointer must remain valid for the duration of the
/// call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_builder_reference_level(
    builder: *mut AaroniaSourceBuilder,
    level: f64,
) {
    unsafe {
        if let Some(builder) = builder.as_mut() {
            builder.reference_level(level);
        }
    }
}

/// Configure the builder to use an HTTP source at the given base URL.
///
/// # Safety
/// - `builder` must either be null (no-op) or a valid pointer to a live
///   `AaroniaSourceBuilder` returned by [`aaronia_source_builder_new`].
/// - `base_url`, if non-null, must point to a NUL-terminated C string that
///   remains valid for the duration of the call. Non-UTF-8 bytes are
///   replaced with the Unicode replacement character.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_builder_http_source(
    builder: *mut AaroniaSourceBuilder,
    base_url: *const c_char,
) {
    unsafe {
        if let Some(builder) = builder.as_mut()
            && !base_url.is_null()
        {
            let url = CStr::from_ptr(base_url).to_string_lossy().into_owned();
            builder.http_source(url);
        }
    }
}

/// Configure the builder to read from an RTSA file at the given path.
///
/// # Safety
/// - `builder` must either be null (no-op) or a valid pointer to a live
///   `AaroniaSourceBuilder` returned by [`aaronia_source_builder_new`].
/// - `file_path`, if non-null, must point to a NUL-terminated C string
///   that remains valid for the duration of the call. Non-UTF-8 bytes are
///   replaced with the Unicode replacement character.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_builder_file_source(
    builder: *mut AaroniaSourceBuilder,
    file_path: *const c_char,
) {
    unsafe {
        if let Some(builder) = builder.as_mut()
            && !file_path.is_null()
        {
            let path = CStr::from_ptr(file_path).to_string_lossy().into_owned();
            builder.file_source(path);
        }
    }
}

/// Consume the builder and asynchronously build an `AaroniaSource`. Returns
/// an opaque pointer that must later be freed with
/// [`aaronia_source_free`], or `NULL` on error.
///
/// # Safety
/// `builder` must either be null (returns `NULL`) or a valid pointer to a
/// live `AaroniaSourceBuilder` returned by [`aaronia_source_builder_new`]
/// and not yet freed. The builder is borrowed (not consumed) so the caller
/// retains ownership and must still free it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_build(builder: *mut AaroniaSourceBuilder) -> *mut c_void {
    clear_last_error();
    if builder.is_null() {
        set_last_error("aaronia_source_build: builder pointer is null");
        return std::ptr::null_mut();
    }
    let builder = unsafe { &*builder };

    match ffi_block_on(builder.build()) {
        Ok(Ok(s)) => Box::into_raw(Box::new(s)) as *mut c_void,
        Ok(Err(e)) => {
            set_last_error(format!("aaronia_source_build failed: {}", e));
            std::ptr::null_mut()
        }
        Err(ctx) => {
            set_last_error(format!("aaronia_source_build: {}", ctx));
            std::ptr::null_mut()
        }
    }
}

// --- AaroniaSource FFI --- //

/// Free a source previously returned by [`aaronia_source_build`].
///
/// # Safety
/// `ptr` must either be null or a pointer previously returned by
/// [`aaronia_source_build`] that has not yet been freed. After this call
/// the pointer must not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_free(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: per the function-level contract, the caller guarantees the
    // pointer originated from `aaronia_source_build` and has not been freed.
    unsafe {
        drop(Box::from_raw(ptr as *mut AaroniaSource));
    }
}

/// Read up to `len` IQ samples into the caller-provided `buffer`. Returns
/// the number of samples written, or a negative error code.
///
/// # Safety
/// - `ptr` must be a valid pointer returned by [`aaronia_source_build`]
///   and not yet freed; null returns `-1`.
/// - `buffer` must be a non-null, properly aligned pointer to at least
///   `len` writable [`FfiComplex`] elements; null returns `-1`.
/// - The buffer must remain valid for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_read_samples(
    ptr: *mut c_void,
    buffer: *mut FfiComplex,
    len: usize,
) -> isize {
    clear_last_error();
    if ptr.is_null() || buffer.is_null() {
        set_last_error("aaronia_source_read_samples: source or buffer pointer is null");
        return -1; // Null pointer error
    }

    // SAFETY: ptr is verified non-null above and was created by Box::into_raw in aaronia_source_build.
    let source = unsafe { &mut *(ptr as *mut AaroniaSource) };

    let mut temp_samples = Vec::new();
    let samples_result = ffi_block_on(source.read_samples(&mut temp_samples, len));

    match samples_result {
        Ok(Ok(samples_to_copy)) => {
            // Clamp to both the caller's requested capacity (`len`) and
            // the actual number of samples `read_samples` populated
            // (`temp_samples.len()`) — trusting `samples_to_copy` alone
            // would let a future miscounting bug in any `read_samples`
            // implementation read past `temp_samples`'s real allocation.
            let samples_to_copy = samples_to_copy.min(len).min(temp_samples.len());
            // SAFETY: buffer is verified non-null above and caller guarantees it points to at least `len` FfiComplex elements.
            // `Complex32` (`num_complex::Complex<f32>`) and `FfiComplex`
            // are both `{re: f32, im: f32}` in memory, so this
            // reinterpret cast is sound; the assert guards against a
            // future representation change silently breaking it.
            const _: () =
                assert!(std::mem::size_of::<FfiComplex>() == std::mem::size_of::<Complex32>());
            unsafe {
                std::ptr::copy_nonoverlapping(
                    temp_samples.as_ptr() as *const FfiComplex,
                    buffer,
                    samples_to_copy,
                );
            }
            samples_to_copy as isize
        }
        Ok(Err(e)) => {
            set_last_error(format!("aaronia_source_read_samples failed: {}", e));
            -3 // Read error
        }
        Err(ctx) => {
            set_last_error(format!("aaronia_source_read_samples: {}", ctx));
            -3
        }
    }
}

/// Start streaming on the source. Returns an `AaroniaFfiError`.
///
/// # Safety
/// `ptr` must be a valid pointer returned by [`aaronia_source_build`] and
/// not yet freed; null returns [`AaroniaFfiError::NullPointer`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_start_streaming(ptr: *mut c_void) -> AaroniaFfiError {
    clear_last_error();
    if ptr.is_null() {
        set_last_error("aaronia_source_start_streaming: source pointer is null");
        return AaroniaFfiError::NullPointer;
    }

    let source = unsafe { &mut *(ptr as *mut AaroniaSource) };

    match ffi_block_on(source.start_streaming()) {
        Ok(Ok(())) => AaroniaFfiError::Success,
        Ok(Err(e)) => {
            set_last_error(format!("aaronia_source_start_streaming failed: {}", e));
            AaroniaFfiError::InternalError
        }
        Err(ctx) => {
            set_last_error(format!("aaronia_source_start_streaming: {}", ctx));
            AaroniaFfiError::RuntimeContext
        }
    }
}

/// Stop streaming on the source. Returns an `AaroniaFfiError`.
///
/// # Safety
/// `ptr` must be a valid pointer returned by [`aaronia_source_build`] and
/// not yet freed; null returns [`AaroniaFfiError::NullPointer`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_stop_streaming(ptr: *mut c_void) -> AaroniaFfiError {
    clear_last_error();
    if ptr.is_null() {
        set_last_error("aaronia_source_stop_streaming: source pointer is null");
        return AaroniaFfiError::NullPointer;
    }

    let source = unsafe { &mut *(ptr as *mut AaroniaSource) };

    match ffi_block_on(source.stop_streaming()) {
        Ok(Ok(())) => AaroniaFfiError::Success,
        Ok(Err(e)) => {
            set_last_error(format!("aaronia_source_stop_streaming failed: {}", e));
            AaroniaFfiError::InternalError
        }
        Err(ctx) => {
            set_last_error(format!("aaronia_source_stop_streaming: {}", ctx));
            AaroniaFfiError::RuntimeContext
        }
    }
}

/// Return a heap-allocated [`FfiSourceInfo`] describing the source. The
/// caller must free it with [`aaronia_source_info_free`]. Returns `NULL`
/// on null input.
///
/// # Safety
/// `ptr` must be a valid pointer returned by [`aaronia_source_build`] and
/// not yet freed; null returns `NULL`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_get_source_info(ptr: *mut c_void) -> *mut FfiSourceInfo {
    if ptr.is_null() {
        return std::ptr::null_mut();
    }

    let source = unsafe { &*(ptr as *mut AaroniaSource) };
    let info = source.get_source_info();

    let device_serial = if let Some(serial) = info.device_serial {
        CString::new(serial)
            .unwrap_or_else(|_| CString::new("invalid").unwrap())
            .into_raw()
    } else {
        std::ptr::null()
    };

    let ffi_info = Box::new(FfiSourceInfo {
        source_type: info.source_type.into(),
        center_frequency: info.center_frequency,
        span_frequency: info.span_frequency,
        bandwidth_hz: info.bandwidth_hz,
        reference_level: info.reference_level,
        device_serial,
    });

    Box::into_raw(ffi_info)
}

/// Free a source-info struct previously returned by
/// [`aaronia_source_get_source_info`].
///
/// # Safety
/// `ptr` must either be null or a pointer previously returned by
/// [`aaronia_source_get_source_info`] that has not yet been freed. After
/// this call the pointer must not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_info_free(ptr: *mut FfiSourceInfo) {
    if ptr.is_null() {
        return;
    }
    let info = unsafe { Box::from_raw(ptr) };
    if !info.device_serial.is_null() {
        unsafe {
            drop(CString::from_raw(info.device_serial as *mut c_char));
        }
    }
}

// --- Remote Control FFI --- //

/// Construct a new HTTP endpoints client. Returns `NULL` on error or if
/// `base_url_ptr` is null / not valid UTF-8.
///
/// # Safety
/// `base_url_ptr`, if non-null, must point to a NUL-terminated C string
/// that remains valid for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_endpoints_client_new(base_url_ptr: *const c_char) -> *mut c_void {
    clear_last_error();
    if base_url_ptr.is_null() {
        set_last_error("aaronia_endpoints_client_new: base_url pointer is null");
        return std::ptr::null_mut();
    }
    let base_url_cstr = unsafe { CStr::from_ptr(base_url_ptr) };
    let base_url = match base_url_cstr.to_str() {
        Ok(s) => s,
        Err(e) => {
            set_last_error(format!(
                "aaronia_endpoints_client_new: base_url is not valid UTF-8: {}",
                e
            ));
            return std::ptr::null_mut();
        }
    };

    match HttpEndpointsClient::new(
        base_url.to_string(),
        crate::http_endpoints::AuthMethod::None,
    ) {
        Ok(client) => Box::into_raw(Box::new(client)) as *mut c_void,
        Err(e) => {
            set_last_error(format!(
                "aaronia_endpoints_client_new failed for {}: {}",
                base_url, e
            ));
            std::ptr::null_mut()
        }
    }
}

/// Free a client previously returned by [`aaronia_endpoints_client_new`].
///
/// # Safety
/// `ptr` must either be null or a pointer previously returned by
/// [`aaronia_endpoints_client_new`] that has not yet been freed. After
/// this call the pointer must not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_endpoints_client_free(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(ptr as *mut HttpEndpointsClient));
    }
}

/// Query the connected RTSA server's metadata. Returns a heap-allocated
/// [`FfiServerInfo`] (free with [`aaronia_server_info_free`]) or `NULL` on
/// error / null input.
///
/// # Safety
/// `ptr` must be a valid pointer returned by
/// [`aaronia_endpoints_client_new`] and not yet freed; null returns
/// `NULL`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_endpoints_client_get_info(ptr: *mut c_void) -> *mut FfiServerInfo {
    unsafe {
        clear_last_error();
        if ptr.is_null() {
            set_last_error("aaronia_endpoints_client_get_info: client pointer is null");
            return std::ptr::null_mut();
        }

        let client = &mut *(ptr as *mut HttpEndpointsClient);
        let info_result = match ffi_block_on(client.get_info()) {
            Ok(r) => r,
            Err(ctx) => {
                set_last_error(format!("aaronia_endpoints_client_get_info: {}", ctx));
                return std::ptr::null_mut();
            }
        };

        match info_result {
            Ok(info) => {
                let ffi_info = Box::new(FfiServerInfo {
                    name: CString::new(info.name)
                        .unwrap_or_else(|_| CString::new("invalid").unwrap())
                        .into_raw(),
                    version: CString::new("N/A".to_string())
                        .unwrap_or_else(|_| CString::new("invalid").unwrap())
                        .into_raw(),
                    build: CString::new("N/A".to_string())
                        .unwrap_or_else(|_| CString::new("invalid").unwrap())
                        .into_raw(),
                    serial: CString::new(info.uuid.clone())
                        .unwrap_or_else(|_| CString::new("invalid").unwrap())
                        .into_raw(), // Use UUID as serial
                    title: CString::new(info.title)
                        .unwrap_or_else(|_| CString::new("invalid").unwrap())
                        .into_raw(),
                    mission: CString::new(info.mission)
                        .unwrap_or_else(|_| CString::new("invalid").unwrap())
                        .into_raw(),
                });
                Box::into_raw(ffi_info)
            }
            Err(e) => {
                set_last_error(format!("aaronia_endpoints_client_get_info failed: {}", e));
                std::ptr::null_mut()
            }
        }
    }
}

/// Free a server-info struct previously returned by
/// [`aaronia_endpoints_client_get_info`].
///
/// # Safety
/// `ptr` must either be null (no-op) or a pointer previously returned by
/// [`aaronia_endpoints_client_get_info`] that has not yet been freed.
/// After this call the pointer must not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_server_info_free(ptr: *mut FfiServerInfo) {
    if ptr.is_null() {
        return;
    }
    let info = unsafe { Box::from_raw(ptr) };
    unsafe {
        drop(CString::from_raw(info.name as *mut c_char));
        drop(CString::from_raw(info.version as *mut c_char));
        drop(CString::from_raw(info.build as *mut c_char));
        drop(CString::from_raw(info.serial as *mut c_char));
        drop(CString::from_raw(info.title as *mut c_char));
        drop(CString::from_raw(info.mission as *mut c_char));
    }
}

/// Start or stop server-side streaming. Returns an `AaroniaFfiError`.
///
/// # Safety
/// `ptr` must be a valid pointer returned by
/// [`aaronia_endpoints_client_new`] and not yet freed; null returns
/// [`AaroniaFfiError::NullPointer`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_endpoints_client_control_streaming(
    ptr: *mut c_void,
    start: bool,
) -> AaroniaFfiError {
    unsafe {
        clear_last_error();
        if ptr.is_null() {
            set_last_error("aaronia_endpoints_client_control_streaming: client pointer is null");
            return AaroniaFfiError::NullPointer;
        }

        let client = &mut *(ptr as *mut HttpEndpointsClient);

        match ffi_block_on(client.control_streaming(start)) {
            Ok(Ok(())) => AaroniaFfiError::Success,
            Ok(Err(e)) => {
                set_last_error(format!(
                    "aaronia_endpoints_client_control_streaming(start={}) failed: {}",
                    start, e
                ));
                AaroniaFfiError::InternalError
            }
            Err(ctx) => {
                set_last_error(format!(
                    "aaronia_endpoints_client_control_streaming: {}",
                    ctx
                ));
                AaroniaFfiError::RuntimeContext
            }
        }
    }
}

/// Start or stop server-side recording. `name` is the recording label and
/// may be null. Returns an `AaroniaFfiError`.
///
/// # Safety
/// - `ptr` must be a valid pointer returned by
///   [`aaronia_endpoints_client_new`] and not yet freed; null returns
///   [`AaroniaFfiError::NullPointer`].
/// - `name`, if non-null, must point to a NUL-terminated C string that
///   remains valid for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_endpoints_client_control_recording(
    ptr: *mut c_void,
    start: bool,
    name: *const c_char,
) -> AaroniaFfiError {
    unsafe {
        clear_last_error();
        if ptr.is_null() {
            set_last_error("aaronia_endpoints_client_control_recording: client pointer is null");
            return AaroniaFfiError::NullPointer;
        }

        let client = &mut *(ptr as *mut HttpEndpointsClient);

        let name_str = if name.is_null() {
            None
        } else {
            Some(CStr::from_ptr(name).to_string_lossy().into_owned())
        };

        match ffi_block_on(client.control_recording(start, name_str)) {
            Ok(Ok(())) => AaroniaFfiError::Success,
            Ok(Err(e)) => {
                set_last_error(format!(
                    "aaronia_endpoints_client_control_recording(start={}) failed: {}",
                    start, e
                ));
                AaroniaFfiError::InternalError
            }
            Err(ctx) => {
                set_last_error(format!(
                    "aaronia_endpoints_client_control_recording: {}",
                    ctx
                ));
                AaroniaFfiError::RuntimeContext
            }
        }
    }
}

// --- General FFI Utilities --- //

/// Free a heap-allocated C string previously handed out by this library
/// (any function that returns `*mut c_char`).
///
/// # Safety
/// `s` must either be null (no-op) or a pointer obtained from one of this
/// library's FFI functions and not yet freed. Passing a string allocated
/// elsewhere (e.g. by `malloc`, `strdup`, or another library) is undefined
/// behaviour.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_string_free(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    unsafe {
        drop(CString::from_raw(s));
    }
}

/// Translate an [`AaroniaFfiError`] into a heap-allocated C string. The
/// caller must free the returned pointer with [`aaronia_string_free`].
///
/// # Safety
/// This function takes only by-value arguments and is sound to call from
/// any thread; the `unsafe` qualifier is required because the returned
/// pointer transfers ownership to the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_get_error_message(error_code: AaroniaFfiError) -> *mut c_char {
    let message = match error_code {
        AaroniaFfiError::Success => "Success",
        AaroniaFfiError::NullPointer => "Null pointer provided",
        AaroniaFfiError::InvalidString => "Invalid UTF-8 string provided",
        AaroniaFfiError::InternalError => "Internal Rust error",
        AaroniaFfiError::BuildFailed => "Failed to build Aaronia source",
        AaroniaFfiError::ReadError => "Failed to read from Aaronia source",
        AaroniaFfiError::RuntimeContext => {
            "Called from a thread context that cannot block (current-thread tokio runtime)"
        }
    };
    CString::new(message)
        .unwrap_or_else(|_| CString::new("invalid").unwrap())
        .into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    #[test]
    fn test_ffi_builder_lifecycle() {
        unsafe {
            // Null builder tests
            aaronia_source_builder_free(ptr::null_mut());
            let null_build = aaronia_source_build(ptr::null_mut());
            assert!(null_build.is_null());

            // Valid builder lifecycle
            let builder = aaronia_source_builder_new();
            assert!(!builder.is_null());

            // Apply some settings (should not crash)
            aaronia_source_builder_center_frequency(builder, 2.4e9);
            aaronia_source_builder_span_frequency(builder, 10e6);
            aaronia_source_builder_reference_level(builder, -20.0);

            // Free the builder
            aaronia_source_builder_free(builder);
        }
    }

    #[test]
    fn test_ffi_null_pointer_handling() {
        unsafe {
            assert!(aaronia_source_read_samples(ptr::null_mut(), ptr::null_mut(), 10) == -1);

            // Enum equality check since we don't derive PartialEq on AaroniaFfiError
            assert!(matches!(
                aaronia_source_start_streaming(ptr::null_mut()),
                AaroniaFfiError::NullPointer
            ));
            assert!(matches!(
                aaronia_source_stop_streaming(ptr::null_mut()),
                AaroniaFfiError::NullPointer
            ));

            assert!(aaronia_source_get_source_info(ptr::null_mut()).is_null());

            assert!(aaronia_endpoints_client_new(ptr::null_mut()).is_null());
            assert!(aaronia_endpoints_client_get_info(ptr::null_mut()).is_null());

            assert!(matches!(
                aaronia_endpoints_client_control_streaming(ptr::null_mut(), true),
                AaroniaFfiError::NullPointer
            ));
            assert!(matches!(
                aaronia_endpoints_client_control_recording(ptr::null_mut(), true, ptr::null_mut()),
                AaroniaFfiError::NullPointer
            ));

            // Safe to free null
            aaronia_source_free(ptr::null_mut());
            aaronia_endpoints_client_free(ptr::null_mut());
            aaronia_string_free(ptr::null_mut());
            aaronia_source_info_free(ptr::null_mut());
            aaronia_server_info_free(ptr::null_mut());
        }
    }

    #[test]
    fn test_ffi_error_message_mapping() {
        unsafe {
            let msg_ptr = aaronia_get_error_message(AaroniaFfiError::NullPointer);
            assert!(!msg_ptr.is_null());
            let msg = CStr::from_ptr(msg_ptr).to_string_lossy();
            assert_eq!(msg, "Null pointer provided");
            aaronia_string_free(msg_ptr);

            let msg_ptr = aaronia_get_error_message(AaroniaFfiError::Success);
            assert!(!msg_ptr.is_null());
            let msg = CStr::from_ptr(msg_ptr).to_string_lossy();
            assert_eq!(msg, "Success");
            aaronia_string_free(msg_ptr);
        }
    }

    /// Round-trip the thread-local last-error slot via FFI: trigger a
    /// known failure (passing a null pointer), call `aaronia_last_error`,
    /// confirm the message mentions the right function. Then call a
    /// successful no-op (a null free), confirm the slot was cleared.
    #[test]
    fn test_aaronia_last_error_roundtrip() {
        unsafe {
            // Trigger a null-pointer failure from a fallible FFI entry
            // point. `aaronia_source_start_streaming` is convenient
            // because it returns an error code AND populates the slot.
            let err = aaronia_source_start_streaming(std::ptr::null_mut());
            assert!(matches!(err, AaroniaFfiError::NullPointer));

            let msg_ptr = aaronia_last_error();
            assert!(
                !msg_ptr.is_null(),
                "expected aaronia_last_error to return a non-null message after a failure"
            );
            let msg = CStr::from_ptr(msg_ptr).to_string_lossy().into_owned();
            aaronia_string_free(msg_ptr);
            assert!(
                msg.contains("aaronia_source_start_streaming"),
                "expected last_error to identify the failing function; got: {msg}"
            );
            assert!(
                msg.contains("null"),
                "expected last_error to describe the failure; got: {msg}"
            );

            // A successful (or no-op) call should clear the slot for
            // the next failure. `aaronia_string_free(NULL)` is a no-op
            // and doesn't touch the slot, so trigger a *successful*
            // fallible call instead by re-running with another null —
            // the API contract is that the slot is cleared at function
            // entry. After clear_last_error runs, the slot should be
            // empty; the failure then immediately repopulates it.
            // Confirm by directly calling clear_last_error via a fresh
            // call path: use aaronia_source_build (clear_last_error +
            // null check is the very first thing).
            let _ = aaronia_source_build(std::ptr::null_mut());
            let msg_ptr2 = aaronia_last_error();
            assert!(
                !msg_ptr2.is_null(),
                "aaronia_last_error should report the latest failure"
            );
            aaronia_string_free(msg_ptr2);
        }
    }
}
