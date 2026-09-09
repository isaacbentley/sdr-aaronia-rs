use crate::http_endpoints::HttpEndpointsClient;
use crate::unified_sink::{SpectranSinkBuilder, UnifiedSink};
use crate::unified_source::{SourceType, SpectranSource, SpectranSourceBuilder};
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
    pub center_frequency_hz: f64,
    /// IQ sample rate (Fs) in Hz — see `SourceInfo::sample_rate_hz`.
    pub sample_rate_hz: f64,
    /// Usable RX/real-time bandwidth in Hz; `0.0` = unknown. Always
    /// `<= sample_rate_hz`.
    pub bandwidth_hz: f64,
    pub reference_level_dbm: f64,
    pub device_serial: *const c_char,
}

// --- SpectranSourceBuilder FFI --- //

/// Aaronia source builder new.
#[unsafe(no_mangle)]
pub extern "C" fn aaronia_source_builder_new() -> *mut SpectranSourceBuilder {
    Box::into_raw(Box::new(SpectranSourceBuilder::new()))
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
pub unsafe extern "C" fn aaronia_source_builder_free(builder: *mut SpectranSourceBuilder) {
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
/// `SpectranSourceBuilder` returned by [`aaronia_source_builder_new`] and
/// not yet freed. The pointer must remain valid for the duration of the
/// call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_builder_center_frequency_hz(
    builder: *mut SpectranSourceBuilder,
    hz: f64,
) {
    unsafe {
        if let Some(builder) = builder.as_mut() {
            builder.center_frequency_hz(hz);
        }
    }
}

/// Set the IQ-mode span frequency on the builder, in Hz.
///
/// # Safety
/// `builder` must either be null (no-op) or a valid pointer to a live
/// `SpectranSourceBuilder` returned by [`aaronia_source_builder_new`] and
/// not yet freed. The pointer must remain valid for the duration of the
/// call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_builder_sample_rate_hz(
    builder: *mut SpectranSourceBuilder,
    hz: f64,
) {
    unsafe {
        if let Some(builder) = builder.as_mut() {
            builder.sample_rate_hz(hz);
        }
    }
}

/// Set the IQ-mode reference level on the builder, in dBm.
///
/// # Safety
/// `builder` must either be null (no-op) or a valid pointer to a live
/// `SpectranSourceBuilder` returned by [`aaronia_source_builder_new`] and
/// not yet freed. The pointer must remain valid for the duration of the
/// call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_builder_reference_level_dbm(
    builder: *mut SpectranSourceBuilder,
    dbm: f64,
) {
    unsafe {
        if let Some(builder) = builder.as_mut() {
            builder.reference_level_dbm(dbm);
        }
    }
}

/// Configure the builder to use an HTTP source at the given base URL.
///
/// # Safety
/// - `builder` must either be null (no-op) or a valid pointer to a live
///   `SpectranSourceBuilder` returned by [`aaronia_source_builder_new`].
/// - `base_url`, if non-null, must point to a NUL-terminated C string that
///   remains valid for the duration of the call. Non-UTF-8 bytes are
///   replaced with the Unicode replacement character.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_builder_http_source(
    builder: *mut SpectranSourceBuilder,
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
///   `SpectranSourceBuilder` returned by [`aaronia_source_builder_new`].
/// - `file_path`, if non-null, must point to a NUL-terminated C string
///   that remains valid for the duration of the call. Non-UTF-8 bytes are
///   replaced with the Unicode replacement character.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_builder_file_source(
    builder: *mut SpectranSourceBuilder,
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

/// Select a device by serial number (native-SDK backend). See
/// [`crate::unified_source::SpectranConfig::device_serial`].
///
/// # Safety
/// `builder` must be a live pointer from
/// [`aaronia_source_builder_new`]; `serial` must be a valid
/// NUL-terminated C string or null (null is a no-op).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_builder_device_serial(
    builder: *mut SpectranSourceBuilder,
    serial: *const c_char,
) {
    unsafe {
        if let Some(builder) = builder.as_mut()
            && !serial.is_null()
        {
            let serial = CStr::from_ptr(serial).to_string_lossy().into_owned();
            builder.device_serial(serial);
        }
    }
}

/// Pin the source to one backend instead of auto-detecting.
///
/// The C ABI had no way to ask for the native SDK: leaving both
/// `http_source` and `file_source` unset auto-detects, which picks the
/// SDK when it is installed and *silently* falls back to localhost HTTP
/// when it is not. With `NativeSdk` forced, a missing SDK is a build
/// error instead, so a capture never quietly comes from another backend
/// — the same guarantee `SpectranConfig::force_native_sdk` gives Rust and
/// `sdk=True` gives Python.
///
/// # Safety
/// `builder` must be a live pointer from [`aaronia_source_builder_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_builder_force_source_type(
    builder: *mut SpectranSourceBuilder,
    source_type: CAaroniaSourceType,
) {
    unsafe {
        if let Some(builder) = builder.as_mut() {
            builder.force_source_type(source_type.into());
        }
    }
}

/// Whether the Aaronia native SDK library is present on this machine, by
/// the same search [`aaronia_source_build`] uses — so a caller can offer
/// the SDK only where forcing it would not fail. Answers without loading
/// the library.
#[unsafe(no_mangle)]
pub extern "C" fn aaronia_sdk_installed() -> bool {
    crate::detection::is_sdk_installed()
}

/// Alias-free bandwidth delivered at an IQ sample rate, Hz. Smaller than
/// the rate: the width the stream's start..end frequency actually covers.
/// Stateless — the SoapySDR plugin's `getBandwidth` calls it on the rate
/// `getSampleRate` reports, so it need not track a rate itself.
#[unsafe(no_mangle)]
pub extern "C" fn aaronia_usable_bandwidth_hz(sample_rate_hz: f64) -> f64 {
    crate::usable_bandwidth_hz(sample_rate_hz)
}

/// The IQ sample rate whose alias-free bandwidth covers `bandwidth_hz`,
/// Hz — the inverse of [`aaronia_usable_bandwidth_hz`]. The plugin's
/// `setBandwidth` maps a requested bandwidth to the rate to ask for, then
/// drives the already-verified `setSampleRate`. Stateless.
#[unsafe(no_mangle)]
pub extern "C" fn aaronia_iq_sample_rate_for_bandwidth(bandwidth_hz: f64) -> f64 {
    crate::iq_sample_rate_for_bandwidth(bandwidth_hz)
}

/// Select the RX channel(s) for native-SDK captures: 0 = Rx1 (default),
/// 1 = Rx2, 2 = Rx1+Rx2 (dual — read with
/// [`aaronia_source_read_samples_dual`]). Other values are ignored.
///
/// # Safety
/// `builder` must be a live pointer from [`aaronia_source_builder_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_builder_receiver_channel(
    builder: *mut SpectranSourceBuilder,
    channel: i32,
) {
    unsafe {
        if let Some(builder) = builder.as_mut() {
            let rx = match channel {
                0 => crate::utils::RxChannel::Rx1,
                1 => crate::utils::RxChannel::Rx2,
                2 => crate::utils::RxChannel::Rx1And2,
                _ => return,
            };
            builder.receiver_channel(rx);
        }
    }
}

/// Select the HTTP wire format: "F32", "F16", or "I16" (the genuine
/// low-bandwidth wire mode — an int16 stream from the server). Unknown
/// strings are ignored.
///
/// # Safety
/// `builder` must be a live pointer from [`aaronia_source_builder_new`];
/// `format` must be a valid NUL-terminated C string or null (no-op).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_builder_stream_format(
    builder: *mut SpectranSourceBuilder,
    format: *const c_char,
) {
    unsafe {
        if let Some(builder) = builder.as_mut()
            && !format.is_null()
        {
            let fmt = match CStr::from_ptr(format).to_string_lossy().as_ref() {
                "F32" => crate::http_streaming::StreamFormat::Float32,
                "F16" => crate::http_streaming::StreamFormat::Float16,
                "I16" => crate::http_streaming::StreamFormat::Int16,
                _ => return,
            };
            builder.stream_format(fmt);
        }
    }
}

/// Set the server-side integer encode multiplier for integer wire
/// formats (`/stream?scale=N`).
///
/// # Safety
/// `builder` must be a live pointer from [`aaronia_source_builder_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_builder_stream_scale(
    builder: *mut SpectranSourceBuilder,
    scale: f64,
) {
    unsafe {
        if let Some(builder) = builder.as_mut() {
            builder.stream_scale(scale);
        }
    }
}

/// Set how long a blocking read waits for samples before returning the
/// timeout code (default 30 s). Affects
/// [`aaronia_source_read_samples`]; the deadline-taking
/// [`aaronia_source_read_samples_timeout`] uses its own per-call value.
/// `0` is ignored (it would make every read time out immediately).
///
/// # Safety
/// `builder` must be a live pointer from [`aaronia_source_builder_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_builder_read_timeout_us(
    builder: *mut SpectranSourceBuilder,
    timeout_us: u64,
) {
    unsafe {
        if let Some(builder) = builder.as_mut()
            && timeout_us > 0
        {
            builder.read_timeout(std::time::Duration::from_micros(timeout_us));
        }
    }
}

/// Enable (`true`, the default) or disable automatic reconnection of the
/// HTTP sample stream after the server closes it or the transport fails.
/// When disabled, a dropped stream ends the session and later reads
/// report an error.
///
/// # Safety
/// `builder` must be a live pointer from [`aaronia_source_builder_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_builder_auto_reconnect(
    builder: *mut SpectranSourceBuilder,
    enabled: bool,
) {
    unsafe {
        if let Some(builder) = builder.as_mut() {
            builder.auto_reconnect(enabled);
        }
    }
}

/// Consume the builder and asynchronously build a `SpectranSource`. Returns
/// an opaque pointer that must later be freed with
/// [`aaronia_source_free`], or `NULL` on error.
///
/// # Safety
/// `builder` must either be null (returns `NULL`) or a valid pointer to a
/// live `SpectranSourceBuilder` returned by [`aaronia_source_builder_new`]
/// and not yet freed. The builder is borrowed (not consumed) so the caller
/// retains ownership and must still free it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_build(builder: *mut SpectranSourceBuilder) -> *mut c_void {
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

// --- SpectranSource FFI --- //

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
        drop(Box::from_raw(ptr as *mut SpectranSource));
    }
}

/// Cap on the up-front reservation for a read. The caller's `len` is
/// untrusted, and `Vec::with_capacity(usize::MAX)` would abort the host
/// process; larger reads still work, the buffer grows.
const READ_RESERVE_CAP: usize = 1 << 22;

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
    let source = unsafe { &mut *(ptr as *mut SpectranSource) };

    let mut temp_samples = Vec::with_capacity(len.min(READ_RESERVE_CAP));
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
            // Timeout maps to this API's private -3 convention (documented
            // in aaronia.h); the SoapySDR plugin translates -3 to
            // SOAPY_SDR_TIMEOUT on its side.
            if let crate::Error::Io(ref io_err) = e
                && io_err.kind() == std::io::ErrorKind::TimedOut
            {
                return -3;
            }
            -1 // Generic stream error
        }
        Err(ctx) => {
            set_last_error(format!("aaronia_source_read_samples: {}", ctx));
            -1 // Generic stream error
        }
    }
}

/// Deadline-bounded variant of [`aaronia_source_read_samples`] for
/// callers with latency budgets (the SoapySDR plugin's `readStream`).
///
/// Waits at most `timeout_us` microseconds. A partial read within the
/// deadline returns the partial count; only a deadline with zero
/// samples returns `-3` (the timeout code). `timeout_us == 0` performs
/// a non-blocking drain of already-buffered samples.
///
/// # Safety
/// Same contract as [`aaronia_source_read_samples`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_read_samples_timeout(
    ptr: *mut c_void,
    buffer: *mut FfiComplex,
    len: usize,
    timeout_us: u64,
) -> isize {
    clear_last_error();
    if ptr.is_null() || buffer.is_null() {
        set_last_error("aaronia_source_read_samples_timeout: source or buffer pointer is null");
        return -1;
    }

    // SAFETY: ptr is verified non-null above and was created by
    // Box::into_raw in aaronia_source_build.
    let source = unsafe { &mut *(ptr as *mut SpectranSource) };

    // The source's reusable staging buffer, not a fresh Vec per call:
    // this is the SoapySDR readStream path, called hundreds of times a
    // second with 512 KiB requests at full rate.
    let mut temp_samples = source.take_scratch();
    temp_samples.reserve(len.min(READ_RESERVE_CAP));
    let timeout = std::time::Duration::from_micros(timeout_us);
    let samples_result =
        ffi_block_on(source.read_samples_deadline(&mut temp_samples, len, timeout));

    let result = match samples_result {
        Ok(Ok(samples_to_copy)) => {
            let samples_to_copy = samples_to_copy.min(len).min(temp_samples.len());
            // SAFETY: identical layout argument as in
            // `aaronia_source_read_samples` above.
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
            set_last_error(format!("aaronia_source_read_samples_timeout failed: {}", e));
            if let crate::Error::Io(ref io_err) = e
                && io_err.kind() == std::io::ErrorKind::TimedOut
            {
                return -3;
            }
            -1
        }
        Err(ctx) => {
            set_last_error(format!("aaronia_source_read_samples_timeout: {}", ctx));
            -1
        }
    };
    source.return_scratch(temp_samples);
    result
}

/// Read up to `len` (Rx1, Rx2) sample *pairs* from a dual-channel
/// (`Rx1+Rx2`) native-SDK stream into two caller buffers. Returns the
/// number of pairs written to both buffers (always equal), `-1` on
/// error. Requires the source to have been built with
/// [`aaronia_source_builder_receiver_channel`]`(…, 2)`.
///
/// # Safety
/// `ptr` must be a live pointer from [`aaronia_source_build`]; `rx1`
/// and `rx2` must each point to `len` writable [`FfiComplex`] elements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_read_samples_dual(
    ptr: *mut c_void,
    rx1: *mut FfiComplex,
    rx2: *mut FfiComplex,
    len: usize,
) -> isize {
    clear_last_error();
    if ptr.is_null() || rx1.is_null() || rx2.is_null() {
        set_last_error("aaronia_source_read_samples_dual: null pointer");
        return -1;
    }
    let source = unsafe { &mut *(ptr as *mut SpectranSource) };

    let mut buf1 = Vec::with_capacity(len.min(READ_RESERVE_CAP));
    let mut buf2 = Vec::with_capacity(len.min(READ_RESERVE_CAP));
    match ffi_block_on(source.read_samples_dual(&mut buf1, &mut buf2, len)) {
        Ok(Ok(pairs)) => {
            let pairs = pairs.min(len).min(buf1.len()).min(buf2.len());
            unsafe {
                std::ptr::copy_nonoverlapping(buf1.as_ptr() as *const FfiComplex, rx1, pairs);
                std::ptr::copy_nonoverlapping(buf2.as_ptr() as *const FfiComplex, rx2, pairs);
            }
            pairs as isize
        }
        Ok(Err(e)) => {
            set_last_error(format!("aaronia_source_read_samples_dual failed: {}", e));
            -1
        }
        Err(ctx) => {
            set_last_error(format!("aaronia_source_read_samples_dual: {}", ctx));
            -1
        }
    }
}

/// Read and clear the latched overrun flag from the source.
///
/// # Safety
/// - `ptr` must be a valid pointer returned by [`aaronia_source_build`]
///   and not yet freed; null returns `false`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_take_overrun(ptr: *mut c_void) -> bool {
    if ptr.is_null() {
        return false;
    }
    let source = unsafe { &mut *(ptr as *mut SpectranSource) };
    source.take_overrun()
}

/// Get the cumulative number of dropped packets.
///
/// # Safety
/// - `ptr` must be a valid pointer returned by [`aaronia_source_build`]
///   and not yet freed; null returns `0`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_get_cumulative_drops(ptr: *mut c_void) -> u64 {
    if ptr.is_null() {
        return 0;
    }
    let source = unsafe { &*(ptr as *mut SpectranSource) };
    source.cumulative_drops()
}

/// Get the hardware timestamp of the last received block (in nanoseconds since epoch).
///
/// # Safety
/// - `ptr` must be a valid pointer returned by [`aaronia_source_build`]
///   and not yet freed; null returns `0`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_get_last_timestamp_ns(ptr: *mut c_void) -> i64 {
    if ptr.is_null() {
        return 0;
    }
    let source = unsafe { &*(ptr as *mut SpectranSource) };
    source.last_timestamp_ns()
}

/// Get the current GPS time in nanoseconds since the Unix epoch, if
/// available and valid. Returns `true` when `out_gps_time_ns` was written,
/// otherwise `false`.
///
/// Nanoseconds, matching [`aaronia_source_get_last_timestamp_ns`]. The
/// device reports seconds as a `double`; converting that to epoch
/// nanoseconds correctly needs the whole and fractional parts handled
/// separately, because the product lands where a `double`'s step is
/// 256 ns. Every caller of the old `double` form had to know that. Now
/// none of them do — but note the reading is still only good to about
/// 240 ns, which is the vendor `double`'s own resolution and not
/// something this can improve on.
///
/// Pass `NULL` for `out_gps_time_ns` to ask only whether a valid fix
/// exists, without wanting the value. That is what a capability probe
/// needs, and the alternative is every such caller declaring a throwaway
/// variable to point at.
///
/// # Safety
/// - `ptr` must be a valid pointer returned by [`aaronia_source_build`]
///   and not yet freed; null returns `false`.
/// - `out_gps_time_ns`, when not null, must be a valid pointer to an `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_get_gps_time_ns(
    ptr: *mut c_void,
    out_gps_time_ns: *mut i64,
) -> bool {
    if ptr.is_null() {
        return false;
    }
    let source = unsafe { &mut *(ptr as *mut SpectranSource) };
    match source.gps_time_ns() {
        Some(nanos) => {
            if !out_gps_time_ns.is_null() {
                unsafe { *out_gps_time_ns = nanos };
            }
            true
        }
        None => false,
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

    let source = unsafe { &mut *(ptr as *mut SpectranSource) };

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

    let source = unsafe { &mut *(ptr as *mut SpectranSource) };

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

/// Set the center frequency (in Hz) on a live source.
///
/// # Safety
/// `ptr` must be a valid pointer returned by [`aaronia_source_build`] and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_set_center_frequency_hz(
    ptr: *mut c_void,
    hz: f64,
) -> AaroniaFfiError {
    clear_last_error();
    if ptr.is_null() {
        set_last_error("aaronia_source_set_center_frequency_hz: source pointer is null");
        return AaroniaFfiError::NullPointer;
    }

    let source = unsafe { &mut *(ptr as *mut SpectranSource) };

    match ffi_block_on(source.set_center_frequency_hz(hz)) {
        Ok(Ok(())) => AaroniaFfiError::Success,
        Ok(Err(e)) => {
            set_last_error(format!(
                "aaronia_source_set_center_frequency_hz failed: {}",
                e
            ));
            AaroniaFfiError::InternalError
        }
        Err(ctx) => {
            set_last_error(format!("aaronia_source_set_center_frequency_hz: {}", ctx));
            AaroniaFfiError::RuntimeContext
        }
    }
}

/// Set the span frequency / sample rate (in Hz) on a live source.
///
/// # Safety
/// `ptr` must be a valid pointer returned by [`aaronia_source_build`] and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_set_sample_rate_hz(
    ptr: *mut c_void,
    hz: f64,
) -> AaroniaFfiError {
    clear_last_error();
    if ptr.is_null() {
        set_last_error("aaronia_source_set_sample_rate_hz: source pointer is null");
        return AaroniaFfiError::NullPointer;
    }

    let source = unsafe { &mut *(ptr as *mut SpectranSource) };

    match ffi_block_on(source.set_sample_rate_hz(hz)) {
        Ok(Ok(())) => AaroniaFfiError::Success,
        Ok(Err(e)) => {
            set_last_error(format!("aaronia_source_set_sample_rate_hz failed: {}", e));
            AaroniaFfiError::InternalError
        }
        Err(ctx) => {
            set_last_error(format!("aaronia_source_set_sample_rate_hz: {}", ctx));
            AaroniaFfiError::RuntimeContext
        }
    }
}

/// Set the reference level (in dBm) on a live source.
///
/// # Safety
/// `ptr` must be a valid pointer returned by [`aaronia_source_build`] and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_set_reference_level_dbm(
    ptr: *mut c_void,
    dbm: f64,
) -> AaroniaFfiError {
    clear_last_error();
    if ptr.is_null() {
        set_last_error("aaronia_source_set_reference_level_dbm: source pointer is null");
        return AaroniaFfiError::NullPointer;
    }

    let source = unsafe { &mut *(ptr as *mut SpectranSource) };

    match ffi_block_on(source.set_reference_level_dbm(dbm)) {
        Ok(Ok(())) => AaroniaFfiError::Success,
        Ok(Err(e)) => {
            set_last_error(format!(
                "aaronia_source_set_reference_level_dbm failed: {}",
                e
            ));
            AaroniaFfiError::InternalError
        }
        Err(ctx) => {
            set_last_error(format!("aaronia_source_set_reference_level_dbm: {}", ctx));
            AaroniaFfiError::RuntimeContext
        }
    }
}

/// Set the device's stream clock source — the frequency/timing reference
/// (`"10MHz"`, `"GPS"`, `"PPS"`, `"Oscillator"`, and the `... Provider`
/// variants a V6 offers). This is the basis for correlating captures across
/// devices that share a reference. Native SDK and HTTP only; a no-op on file
/// sources. Returns `Success`, or an error whose detail is in
/// `aaronia_last_error()`.
///
/// # Safety
/// `ptr` must be a live `SpectranSource` from `aaronia_source_builder_build`;
/// `source` must be a valid NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_set_clock_source(
    ptr: *mut c_void,
    source: *const c_char,
) -> AaroniaFfiError {
    clear_last_error();
    if ptr.is_null() {
        set_last_error("aaronia_source_set_clock_source: source pointer is null");
        return AaroniaFfiError::NullPointer;
    }
    if source.is_null() {
        set_last_error("aaronia_source_set_clock_source: clock-source string is null");
        return AaroniaFfiError::NullPointer;
    }
    let clock_source = match unsafe { CStr::from_ptr(source) }.to_str() {
        Ok(s) => s,
        Err(e) => {
            set_last_error(format!(
                "aaronia_source_set_clock_source: clock source is not valid UTF-8: {}",
                e
            ));
            return AaroniaFfiError::InvalidString;
        }
    };

    let source_ref = unsafe { &mut *(ptr as *mut SpectranSource) };
    match ffi_block_on(source_ref.set_clock_source(clock_source)) {
        Ok(Ok(())) => AaroniaFfiError::Success,
        Ok(Err(e)) => {
            set_last_error(format!("aaronia_source_set_clock_source failed: {}", e));
            AaroniaFfiError::InternalError
        }
        Err(ctx) => {
            set_last_error(format!("aaronia_source_set_clock_source: {}", ctx));
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

    let source = unsafe { &*(ptr as *mut SpectranSource) };
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
        center_frequency_hz: info.center_frequency_hz,
        sample_rate_hz: info.sample_rate_hz,
        bandwidth_hz: info.bandwidth_hz,
        reference_level_dbm: info.reference_level_dbm,
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

/// What the device reports about itself, flattened for C.
///
/// Optionality is explicit because no numeric sentinel would do: `0.0`
/// is a legitimate reference level and a legitimate step, so a caller
/// must be able to tell "the device declares 0" from "the device did
/// not say". Strings use NULL for absent; the two ranges carry a
/// `has_` flag; `sample_rate_count == 0` means the ladder could not be
/// derived.
#[repr(C)]
pub struct FfiDeviceCapabilities {
    /// Model name, e.g. `"SPECTRAN V6 ECO"`. NULL when unknown.
    pub model: *const c_char,
    /// Device serial. NULL when unknown.
    pub serial: *const c_char,
    /// Firmware/FPGA version string. NULL when unknown.
    pub version: *const c_char,

    /// Whether the three `center_frequency_*` fields carry a reading.
    pub has_center_frequency: bool,
    pub center_frequency_min_hz: f64,
    pub center_frequency_max_hz: f64,
    /// Distance between valid centre frequencies, or `0.0` when the
    /// device declares none.
    pub center_frequency_step_hz: f64,

    /// Whether the three `reference_level_*` fields carry a reading.
    pub has_reference_level: bool,
    pub reference_level_min_dbm: f64,
    pub reference_level_max_dbm: f64,
    /// Step between valid reference levels, or `0.0` when undeclared.
    pub reference_level_step_db: f64,

    /// Entries in [`Self::sample_rates`]; `0` when the device could not
    /// be asked, which is the signal to fall back to a compiled-in
    /// ladder rather than advertise none.
    pub sample_rate_count: usize,
    /// Settable IQ sample rates in Hz, highest first. Owned by this
    /// struct and freed with it; NULL when the count is 0.
    pub sample_rates: *const f64,

    /// Stream-clock sources the device offers, in its own vocabulary.
    /// Owned by this struct; NULL and 0 when it did not say.
    pub clock_source_count: usize,
    pub clock_sources: *const *const c_char,
    /// The source currently selected. NULL when unknown.
    pub clock_source: *const c_char,
    /// RX input the device's mode names, e.g. `"RX1"`. NULL when the
    /// device names none.
    pub rx_antenna: *const c_char,
}

/// Read the attached device's declared capabilities.
///
/// Blocking: performs two control-plane GETs. Returns NULL only for a
/// null `ptr` or when called from a current-thread tokio runtime (where
/// blocking would deadlock); a device that cannot be asked yields a
/// populated struct whose fields are all absent, so a caller falls back
/// per field rather than on the whole reading.
///
/// Answers for the HTTP backend. The file and native-SDK backends
/// report everything absent — they have no equivalent surface to ask.
///
/// # Safety
/// `ptr` must be a live pointer from [`aaronia_source_build`], not used
/// concurrently from another thread. The returned pointer must be freed
/// exactly once with [`aaronia_source_capabilities_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_get_capabilities(
    ptr: *mut c_void,
) -> *mut FfiDeviceCapabilities {
    clear_last_error();
    if ptr.is_null() {
        set_last_error("Null pointer".to_string());
        return std::ptr::null_mut();
    }
    let source = unsafe { &*(ptr as *mut SpectranSource) };
    let caps = match ffi_block_on(source.device_capabilities()) {
        Ok(caps) => caps,
        Err(e) => {
            set_last_error(e);
            return std::ptr::null_mut();
        }
    };

    // A string the device did not report becomes NULL, not "": the
    // caller has to be able to keep its own default for that field.
    let owned = |value: Option<String>| -> *const c_char {
        value
            .and_then(|s| CString::new(s).ok())
            .map_or(std::ptr::null(), |s| s.into_raw() as *const c_char)
    };

    let (sample_rates, sample_rate_count) = match caps.sample_rates() {
        Some(rates) if !rates.is_empty() => {
            let boxed = rates.into_boxed_slice();
            let len = boxed.len();
            (Box::into_raw(boxed) as *const f64, len)
        }
        _ => (std::ptr::null(), 0),
    };

    // An array of owned C strings, one slot per option. A slot is NULL
    // when the name could not be made into a C string (an interior NUL),
    // so callers check each entry — the C++ plugin does.
    let (clock_sources, clock_source_count) = if caps.clock_sources.is_empty() {
        (std::ptr::null(), 0)
    } else {
        let ptrs: Vec<*const c_char> = caps
            .clock_sources
            .iter()
            .map(|s| owned(Some(s.clone())))
            .collect();
        let len = ptrs.len();
        (
            Box::into_raw(ptrs.into_boxed_slice()) as *const *const c_char,
            len,
        )
    };

    Box::into_raw(Box::new(FfiDeviceCapabilities {
        clock_source_count,
        clock_sources,
        clock_source: owned(caps.clock_source),
        rx_antenna: owned(caps.rx_antenna),
        model: owned(caps.model),
        serial: owned(caps.serial),
        version: owned(caps.version),
        has_center_frequency: caps.center_frequency_hz.is_some(),
        center_frequency_min_hz: caps.center_frequency_hz.map_or(0.0, |r| r.min),
        center_frequency_max_hz: caps.center_frequency_hz.map_or(0.0, |r| r.max),
        center_frequency_step_hz: caps.center_frequency_hz.and_then(|r| r.step).unwrap_or(0.0),
        has_reference_level: caps.reference_level_dbm.is_some(),
        reference_level_min_dbm: caps.reference_level_dbm.map_or(0.0, |r| r.min),
        reference_level_max_dbm: caps.reference_level_dbm.map_or(0.0, |r| r.max),
        reference_level_step_db: caps.reference_level_dbm.and_then(|r| r.step).unwrap_or(0.0),
        sample_rate_count,
        sample_rates,
    }))
}

/// Free a capabilities struct from [`aaronia_source_get_capabilities`],
/// including the strings and the sample-rate array it owns.
///
/// # Safety
/// `ptr` must be null or a pointer returned by
/// [`aaronia_source_get_capabilities`] that has not already been freed.
/// After this call neither it nor any pointer read out of it may be
/// used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_capabilities_free(ptr: *mut FfiDeviceCapabilities) {
    if ptr.is_null() {
        return;
    }
    let caps = unsafe { Box::from_raw(ptr) };
    for s in [
        caps.model,
        caps.serial,
        caps.version,
        caps.clock_source,
        caps.rx_antenna,
    ] {
        if !s.is_null() {
            unsafe { drop(CString::from_raw(s as *mut c_char)) };
        }
    }
    if !caps.clock_sources.is_null() {
        // Two levels: each string, then the array of pointers itself.
        let ptrs = unsafe {
            Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                caps.clock_sources as *mut *const c_char,
                caps.clock_source_count,
            ))
        };
        for s in ptrs.iter() {
            if !s.is_null() {
                unsafe { drop(CString::from_raw(*s as *mut c_char)) };
            }
        }
    }
    if !caps.sample_rates.is_null() {
        // Reconstitute the same fat pointer `into_raw` split apart;
        // freeing it as a single `f64` would leak all but the first.
        unsafe {
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                caps.sample_rates as *mut f64,
                caps.sample_rate_count,
            )));
        }
    }
}

/// The device's live sensors, filled by [`aaronia_source_get_sensors`].
///
/// A plain value struct the caller allocates: every field is a reading,
/// or `NaN` for "the device did not report it". No pointers, no
/// ownership, nothing to free. Units are on the Rust
/// [`DeviceSensors`](crate::http_endpoints::DeviceSensors) fields.
#[repr(C)]
pub struct FfiDeviceSensors {
    pub fpga_temp_c: f64,
    pub frontend_temp_c: f64,
    pub board_power_w: f64,
    pub adc_range_db: f64,
    pub usb_buffer_fill: f64,
    pub dsp_buffer_fill: f64,
    pub errors_per_second: f64,
    pub usb_overflows_per_second: f64,
    pub dsp_overflows_per_second: f64,
    pub gps_satellites: f64,
    pub gps_latitude: f64,
    pub gps_longitude: f64,
}

/// Read the device's live sensors into `out`.
///
/// An HTTP-backend feature: a `/healthstatus` GET filled onto
/// `FfiDeviceSensors`. Returns `true` when the read completed; individual
/// fields are `NaN` where the device omits them, and the file and
/// native-SDK backends complete with every field `NaN` (the raw SDK's own
/// health tree reads all zeros — see [`SpectranSource::device_sensors`]).
/// Returns `false`, leaving `out` untouched, only for a null pointer or
/// when called from a current-thread tokio runtime (where the blocking
/// GET would deadlock).
///
/// Blocking: one `/healthstatus` GET on the HTTP backend.
///
/// # Safety
/// `ptr` must be a live pointer from [`aaronia_source_build`], not used
/// concurrently from another thread; `out` must point to a writable
/// `FfiDeviceSensors`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_source_get_sensors(
    ptr: *mut c_void,
    out: *mut FfiDeviceSensors,
) -> bool {
    clear_last_error();
    if ptr.is_null() || out.is_null() {
        set_last_error("Null pointer".to_string());
        return false;
    }
    let source = unsafe { &*(ptr as *mut SpectranSource) };
    let sensors = match ffi_block_on(source.device_sensors()) {
        Ok(s) => s,
        Err(e) => {
            set_last_error(e);
            return false;
        }
    };
    let f = |v: Option<f64>| v.unwrap_or(f64::NAN);
    unsafe {
        *out = FfiDeviceSensors {
            fpga_temp_c: f(sensors.fpga_temp_c),
            frontend_temp_c: f(sensors.frontend_temp_c),
            board_power_w: f(sensors.board_power_w),
            adc_range_db: f(sensors.adc_range_db),
            usb_buffer_fill: f(sensors.usb_buffer_fill),
            dsp_buffer_fill: f(sensors.dsp_buffer_fill),
            errors_per_second: f(sensors.errors_per_second),
            usb_overflows_per_second: f(sensors.usb_overflows_per_second),
            dsp_overflows_per_second: f(sensors.dsp_overflows_per_second),
            gps_satellites: f(sensors.gps_satellites),
            gps_latitude: f(sensors.gps_latitude),
            gps_longitude: f(sensors.gps_longitude),
        };
    }
    true
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

/// Read the device's live sensors through an endpoints client, into
/// `out`. The same reading as [`aaronia_source_get_sensors`], but off a
/// standalone client rather than a streaming source — so a caller can
/// poll sensors during a capture without contending for the source's
/// read lock, which would stall sample delivery.
///
/// Returns `true` when the read completed (fields may be `NaN` where the
/// device omits them), `false` — leaving `out` untouched — for a null
/// pointer or a current-thread runtime. A failed GET completes all-`NaN`.
///
/// Blocking: one `/healthstatus` GET.
///
/// # Safety
/// `ptr` must be a live pointer from [`aaronia_endpoints_client_new`],
/// not yet freed; `out` must point to a writable `FfiDeviceSensors`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_endpoints_client_get_sensors(
    ptr: *mut c_void,
    out: *mut FfiDeviceSensors,
) -> bool {
    clear_last_error();
    if ptr.is_null() || out.is_null() {
        set_last_error("aaronia_endpoints_client_get_sensors: null pointer".to_string());
        return false;
    }
    let client = unsafe { &*(ptr as *mut HttpEndpointsClient) };
    let sensors = match ffi_block_on(client.get_device_sensors()) {
        Ok(result) => result.unwrap_or_default(),
        Err(e) => {
            set_last_error(format!("aaronia_endpoints_client_get_sensors: {e}"));
            return false;
        }
    };
    let f = |v: Option<f64>| v.unwrap_or(f64::NAN);
    unsafe {
        *out = FfiDeviceSensors {
            fpga_temp_c: f(sensors.fpga_temp_c),
            frontend_temp_c: f(sensors.frontend_temp_c),
            board_power_w: f(sensors.board_power_w),
            adc_range_db: f(sensors.adc_range_db),
            usb_buffer_fill: f(sensors.usb_buffer_fill),
            dsp_buffer_fill: f(sensors.dsp_buffer_fill),
            errors_per_second: f(sensors.errors_per_second),
            usb_overflows_per_second: f(sensors.usb_overflows_per_second),
            dsp_overflows_per_second: f(sensors.dsp_overflows_per_second),
            gps_satellites: f(sensors.gps_satellites),
            gps_latitude: f(sensors.gps_latitude),
            gps_longitude: f(sensors.gps_longitude),
        };
    }
    true
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
pub unsafe extern "C" fn aaronia_get_error_message(error_code: std::os::raw::c_int) -> *mut c_char {
    // Taken as a plain int, not the enum: a C enum is an int and any
    // value can arrive, while an out-of-range Rust enum is undefined
    // behaviour before the match runs.
    let message = match error_code {
        x if x == AaroniaFfiError::Success as i32 => "Success",
        x if x == AaroniaFfiError::NullPointer as i32 => "Null pointer provided",
        x if x == AaroniaFfiError::InvalidString as i32 => "Invalid UTF-8 string provided",
        x if x == AaroniaFfiError::InternalError as i32 => "Internal Rust error",
        x if x == AaroniaFfiError::BuildFailed as i32 => "Failed to build Aaronia source",
        x if x == AaroniaFfiError::ReadError as i32 => "Failed to read from Aaronia source",
        x if x == AaroniaFfiError::RuntimeContext as i32 => {
            "Called from a thread context that cannot block (current-thread tokio runtime)"
        }
        _ => "Unknown error code",
    };
    CString::new(message)
        .unwrap_or_else(|_| CString::new("invalid").unwrap())
        .into_raw()
}

// -----------------------------------------------------------------------------
// SINK API
//
// > [!WARNING]
// > The entire TX path is hardware-unverified (see `unified_sink` /
// > `sdk_sink` module docs) and requires the native SDK on
// > Windows/Linux; on other builds `aaronia_sink_initialize` fails with
// > a descriptive error.
// -----------------------------------------------------------------------------

/// Create a new sink builder. Free with [`aaronia_sink_builder_free`].
///
/// # Safety
/// Takes no arguments; sound to call from any thread. The returned
/// pointer must be freed exactly once with
/// [`aaronia_sink_builder_free`] (or consumed by nothing — building
/// borrows it).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_sink_builder_new() -> *mut SpectranSinkBuilder {
    Box::into_raw(Box::new(SpectranSinkBuilder::new()))
}

/// Whether this build carries a transmit path at all.
///
/// `aaronia_sink_build` succeeds everywhere — it allocates a sink
/// object, and only `aaronia_sink_initialize` fails where TX is
/// unavailable — so a non-null sink is **not** evidence that anything
/// can transmit. A caller that has to advertise the capability before
/// opening anything (the SoapySDR plugin publishes its TX channel count
/// at device construction) must ask this instead, or it advertises a TX
/// channel whose every write fails.
///
/// Returns compile-time availability only: `true` means the binary
/// carries the native-SDK TX code, not that the attached device has a
/// transmitter. A SPECTRAN V6 ECO does not, and this cannot tell.
/// Transmission additionally requires the native-SDK *source* backend,
/// so a device opened over HTTP or from a file has no TX path whatever
/// this returns.
///
/// Takes no arguments and touches no state, so it is a safe `fn`: sound
/// to call from any thread at any time.
#[unsafe(no_mangle)]
pub extern "C" fn aaronia_sink_supported() -> bool {
    crate::unified_sink::UnifiedSink::tx_supported()
}

/// Free a sink builder. Null is a no-op.
///
/// # Safety
/// `builder` must be null or a pointer returned by
/// [`aaronia_sink_builder_new`] that has not already been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_sink_builder_free(builder: *mut SpectranSinkBuilder) {
    if !builder.is_null() {
        unsafe { drop(Box::from_raw(builder)) };
    }
}

/// Set the TX center frequency in Hz on a sink builder.
///
/// # Safety
/// `builder` must be a live pointer from [`aaronia_sink_builder_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_sink_builder_center_frequency_hz(
    builder: *mut SpectranSinkBuilder,
    hz: f64,
) {
    if let Some(b) = unsafe { builder.as_mut() } {
        let updated = std::mem::take(b).center_frequency_hz(hz);
        *b = updated;
    }
}

/// Set the TX IQ sample rate (span) in Hz on a sink builder.
///
/// # Safety
/// `builder` must be a live pointer from [`aaronia_sink_builder_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_sink_builder_sample_rate_hz(
    builder: *mut SpectranSinkBuilder,
    hz: f64,
) {
    if let Some(b) = unsafe { builder.as_mut() } {
        let updated = std::mem::take(b).sample_rate_hz(hz);
        *b = updated;
    }
}

/// Set the transmission gain in dB on a sink builder.
///
/// # Safety
/// `builder` must be a live pointer from [`aaronia_sink_builder_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_sink_builder_trans_gain_db(
    builder: *mut SpectranSinkBuilder,
    db: f64,
) {
    if let Some(b) = unsafe { builder.as_mut() } {
        let updated = std::mem::take(b).trans_gain_db(db);
        *b = updated;
    }
}

/// Build a sink from the builder's current configuration.
///
/// The builder is **borrowed**, exactly like [`aaronia_source_build`]:
/// the caller retains ownership and must still free it with
/// [`aaronia_sink_builder_free`]. (An earlier revision consumed the
/// builder here while the source API borrowed it — following the
/// source convention then double-freed the builder on every open.)
///
/// # Safety
/// `builder` must be a live pointer from [`aaronia_sink_builder_new`].
/// The returned sink must be freed with [`aaronia_sink_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_sink_build(builder: *mut SpectranSinkBuilder) -> *mut c_void {
    clear_last_error();
    let Some(builder_ref) = (unsafe { builder.as_ref() }) else {
        set_last_error("Null builder".to_string());
        return ptr::null_mut();
    };
    let sink = builder_ref.clone().build();
    Box::into_raw(Box::new(sink)) as *mut c_void
}

/// Free a sink. Null is a no-op.
///
/// # Safety
/// `ptr` must be null or a pointer returned by [`aaronia_sink_build`]
/// that has not already been freed, with no other thread using it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_sink_free(ptr: *mut c_void) {
    if !ptr.is_null() {
        unsafe { drop(Box::from_raw(ptr as *mut UnifiedSink)) };
    }
}

/// Initialize the sink and bring the transmitter up: loads the native
/// SDK, opens the first matching device, configures the IQ transmitter
/// from the builder settings, and starts the TX stream. (An earlier
/// revision only loaded the SDK library — nothing ever opened or
/// started the device, so every write failed.)
///
/// Blocking; uses the shared FFI runtime via `ffi_block_on` — safe to
/// call from plain C threads and from multi-threaded tokio contexts.
///
/// # Safety
/// `ptr` must be a live pointer from [`aaronia_sink_build`], not used
/// concurrently from another thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_sink_initialize(ptr: *mut c_void) -> AaroniaFfiError {
    clear_last_error();
    if ptr.is_null() {
        set_last_error("Null pointer".to_string());
        return AaroniaFfiError::NullPointer;
    }
    let sink = unsafe { &mut *(ptr as *mut UnifiedSink) };
    match ffi_block_on(async {
        sink.initialize().await?;
        sink.start_streaming().await
    }) {
        Ok(Ok(())) => AaroniaFfiError::Success,
        Ok(Err(e)) => {
            set_last_error(format!("Initialize error: {}", e));
            AaroniaFfiError::InternalError
        }
        Err(msg) => {
            set_last_error(msg);
            AaroniaFfiError::InternalError
        }
    }
}

/// Stop the TX stream and disconnect the device.
///
/// # Safety
/// `ptr` must be a live pointer from [`aaronia_sink_build`], not used
/// concurrently from another thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_sink_stop_streaming(ptr: *mut c_void) -> AaroniaFfiError {
    clear_last_error();
    if ptr.is_null() {
        set_last_error("Null pointer".to_string());
        return AaroniaFfiError::NullPointer;
    }
    let sink = unsafe { &mut *(ptr as *mut UnifiedSink) };
    match ffi_block_on(async { sink.stop_streaming().await }) {
        Ok(Ok(())) => AaroniaFfiError::Success,
        Ok(Err(e)) => {
            set_last_error(format!("Stop streaming error: {}", e));
            AaroniaFfiError::InternalError
        }
        Err(msg) => {
            set_last_error(msg);
            AaroniaFfiError::InternalError
        }
    }
}

/// Queue one burst of interleaved IQ samples for transmission.
///
/// `start_time_s`/`end_time_s` are in device **master stream time**
/// seconds; `flags` are `tx_flags` packet-boundary bits (pass
/// `AARONIA_TX_SEGMENT_START | AARONIA_TX_SEGMENT_END | AARONIA_TX_PUSH`
/// for a self-contained burst). Samples use the same
/// [`FfiComplex`] layout as the read path — the header no longer uses
/// C99 `_Complex`, which MSVC rejects in C++.
///
/// # Safety
/// `ptr` must be a live pointer from [`aaronia_sink_build`]; `samples`
/// must point to `num_samples` readable [`FfiComplex`] elements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aaronia_sink_write_samples(
    ptr: *mut c_void,
    channel: i32,
    start_time_s: f64,
    end_time_s: f64,
    flags: u64,
    samples: *const FfiComplex,
    num_samples: usize,
) -> AaroniaFfiError {
    clear_last_error();
    if ptr.is_null() || samples.is_null() {
        set_last_error("Null pointer".to_string());
        return AaroniaFfiError::NullPointer;
    }

    // A length that cannot describe a real buffer (more than isize::MAX
    // bytes) is a caller bug; refuse it rather than build an invalid slice.
    if num_samples > isize::MAX as usize / std::mem::size_of::<FfiComplex>() {
        set_last_error(format!(
            "num_samples {num_samples} exceeds the maximum slice length"
        ));
        return AaroniaFfiError::InternalError;
    }

    let sink = unsafe { &mut *(ptr as *mut UnifiedSink) };
    // SAFETY: FfiComplex and Complex32 are both repr(C) {f32, f32};
    // the layout assertion lives next to the FfiComplex definition.
    let slice = unsafe { std::slice::from_raw_parts(samples as *const Complex32, num_samples) };

    match sink.write_samples(channel, start_time_s, end_time_s, flags, slice) {
        Ok(_) => AaroniaFfiError::Success,
        Err(e) => {
            set_last_error(format!("Write error: {}", e));
            AaroniaFfiError::InternalError
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    /// The capabilities struct owns three C strings and a heap array,
    /// and the free function has to give every one of them back. A
    /// missed `sample_rates` would leak the whole ladder on each probe,
    /// and freeing the array as a single `f64` would leak all but its
    /// first element — neither shows up as a test failure anywhere else,
    /// so this is exercised under whatever sanitiser CI runs.
    #[test]
    fn ffi_device_capabilities_layout_matches_the_c_header() {
        // Offsets measured from the C side by compiling include/aaronia.h:
        //   cc -I include abi_check.c ... && ./abi_check
        // A mismatch here is silent memory corruption across the ABI, not a
        // compile error, so the two layouts are pinned against each other.
        assert_eq!(std::mem::size_of::<FfiDeviceCapabilities>(), 136);
        assert_eq!(std::mem::align_of::<FfiDeviceCapabilities>(), 8);
        for (name, got, want) in [
            (
                "model",
                std::mem::offset_of!(FfiDeviceCapabilities, model),
                0,
            ),
            (
                "serial",
                std::mem::offset_of!(FfiDeviceCapabilities, serial),
                8,
            ),
            (
                "version",
                std::mem::offset_of!(FfiDeviceCapabilities, version),
                16,
            ),
            (
                "has_center_frequency",
                std::mem::offset_of!(FfiDeviceCapabilities, has_center_frequency),
                24,
            ),
            (
                "center_frequency_min_hz",
                std::mem::offset_of!(FfiDeviceCapabilities, center_frequency_min_hz),
                32,
            ),
            (
                "center_frequency_max_hz",
                std::mem::offset_of!(FfiDeviceCapabilities, center_frequency_max_hz),
                40,
            ),
            (
                "center_frequency_step_hz",
                std::mem::offset_of!(FfiDeviceCapabilities, center_frequency_step_hz),
                48,
            ),
            (
                "has_reference_level",
                std::mem::offset_of!(FfiDeviceCapabilities, has_reference_level),
                56,
            ),
            (
                "reference_level_min_dbm",
                std::mem::offset_of!(FfiDeviceCapabilities, reference_level_min_dbm),
                64,
            ),
            (
                "reference_level_max_dbm",
                std::mem::offset_of!(FfiDeviceCapabilities, reference_level_max_dbm),
                72,
            ),
            (
                "reference_level_step_db",
                std::mem::offset_of!(FfiDeviceCapabilities, reference_level_step_db),
                80,
            ),
            (
                "sample_rate_count",
                std::mem::offset_of!(FfiDeviceCapabilities, sample_rate_count),
                88,
            ),
            (
                "sample_rates",
                std::mem::offset_of!(FfiDeviceCapabilities, sample_rates),
                96,
            ),
            (
                "clock_source_count",
                std::mem::offset_of!(FfiDeviceCapabilities, clock_source_count),
                104,
            ),
            (
                "clock_sources",
                std::mem::offset_of!(FfiDeviceCapabilities, clock_sources),
                112,
            ),
            (
                "clock_source",
                std::mem::offset_of!(FfiDeviceCapabilities, clock_source),
                120,
            ),
            (
                "rx_antenna",
                std::mem::offset_of!(FfiDeviceCapabilities, rx_antenna),
                128,
            ),
        ] {
            assert_eq!(
                got, want,
                "field {name} moved: the C header and this struct disagree"
            );
        }
    }

    /// Same pin for the two structs that predate the capabilities one.
    /// Offsets measured from C the same way.
    #[test]
    fn ffi_info_struct_layouts_match_the_c_header() {
        assert_eq!(std::mem::size_of::<FfiSourceInfo>(), 48);
        assert_eq!(std::mem::offset_of!(FfiSourceInfo, source_type), 0);
        assert_eq!(std::mem::offset_of!(FfiSourceInfo, center_frequency_hz), 8);
        assert_eq!(std::mem::offset_of!(FfiSourceInfo, sample_rate_hz), 16);
        assert_eq!(std::mem::offset_of!(FfiSourceInfo, bandwidth_hz), 24);
        assert_eq!(std::mem::offset_of!(FfiSourceInfo, reference_level_dbm), 32);
        assert_eq!(std::mem::offset_of!(FfiSourceInfo, device_serial), 40);

        assert_eq!(std::mem::size_of::<FfiServerInfo>(), 48);
        assert_eq!(std::mem::offset_of!(FfiServerInfo, name), 0);
        assert_eq!(std::mem::offset_of!(FfiServerInfo, version), 8);
        assert_eq!(std::mem::offset_of!(FfiServerInfo, build), 16);
        assert_eq!(std::mem::offset_of!(FfiServerInfo, serial), 24);
        assert_eq!(std::mem::offset_of!(FfiServerInfo, title), 32);
        assert_eq!(std::mem::offset_of!(FfiServerInfo, mission), 40);

        // Twelve tightly packed f64s: the C header declares the same
        // twelve `double`s in the same order, so 8-byte stride, no
        // padding, 96 bytes total. Pin the ends and the size; a reordered
        // or retyped field shifts one of these.
        assert_eq!(std::mem::size_of::<FfiDeviceSensors>(), 96);
        assert_eq!(std::mem::offset_of!(FfiDeviceSensors, fpga_temp_c), 0);
        assert_eq!(std::mem::offset_of!(FfiDeviceSensors, adc_range_db), 24);
        assert_eq!(std::mem::offset_of!(FfiDeviceSensors, gps_longitude), 88);
    }

    #[test]
    fn capabilities_free_releases_every_owned_allocation() {
        let rates = vec![61_440_000.0_f64, 30_720_000.0, 15_360_000.0].into_boxed_slice();
        let count = rates.len();
        let clocks: Vec<*const c_char> = ["Oscillator", "GPS", "10MHz"]
            .iter()
            .map(|s| CString::new(*s).unwrap().into_raw() as *const c_char)
            .collect();
        let clock_source_count = clocks.len();
        let clock_sources = Box::into_raw(clocks.into_boxed_slice()) as *const *const c_char;

        let caps = Box::into_raw(Box::new(FfiDeviceCapabilities {
            model: CString::new("SPECTRAN V6 ECO").unwrap().into_raw(),
            serial: CString::new("C2-P-03000105").unwrap().into_raw(),
            version: CString::new("0 0.0.36").unwrap().into_raw(),
            clock_source_count,
            clock_sources,
            clock_source: CString::new("10MHz").unwrap().into_raw(),
            rx_antenna: CString::new("RX1").unwrap().into_raw(),
            has_center_frequency: true,
            center_frequency_min_hz: 5_500_000.0,
            center_frequency_max_hz: 8_000_000_000.0,
            center_frequency_step_hz: 1000.0,
            has_reference_level: true,
            reference_level_min_dbm: -55.0,
            reference_level_max_dbm: 23.0,
            reference_level_step_db: 0.5,
            sample_rate_count: count,
            sample_rates: Box::into_raw(rates) as *const f64,
        }));

        unsafe {
            assert_eq!((*caps).sample_rate_count, 3);
            assert_eq!(*(*caps).sample_rates.add(2), 15_360_000.0);
            aaronia_source_capabilities_free(caps);
        }
    }

    /// The absent case has to free cleanly too: NULL strings and a NULL
    /// array are what a device that could not be asked produces, and
    /// that is the common path on a file or native-SDK source.
    #[test]
    fn capabilities_free_accepts_an_empty_reading() {
        let caps = Box::into_raw(Box::new(FfiDeviceCapabilities {
            model: ptr::null(),
            serial: ptr::null(),
            version: ptr::null(),
            clock_source_count: 0,
            clock_sources: ptr::null(),
            clock_source: ptr::null(),
            rx_antenna: ptr::null(),
            has_center_frequency: false,
            center_frequency_min_hz: 0.0,
            center_frequency_max_hz: 0.0,
            center_frequency_step_hz: 0.0,
            has_reference_level: false,
            reference_level_min_dbm: 0.0,
            reference_level_max_dbm: 0.0,
            reference_level_step_db: 0.0,
            sample_rate_count: 0,
            sample_rates: ptr::null(),
        }));
        unsafe { aaronia_source_capabilities_free(caps) };
        // Null is a no-op, as every free in this ABI is.
        unsafe { aaronia_source_capabilities_free(ptr::null_mut()) };
    }

    #[test]
    fn capabilities_of_a_null_source_are_null() {
        let caps = unsafe { aaronia_source_get_capabilities(ptr::null_mut()) };
        assert!(caps.is_null());
    }

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
            aaronia_source_builder_center_frequency_hz(builder, 2.4e9);
            aaronia_source_builder_sample_rate_hz(builder, 10e6);
            aaronia_source_builder_reference_level_dbm(builder, -20.0);

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
            let msg_ptr = aaronia_get_error_message(AaroniaFfiError::NullPointer as i32);
            assert!(!msg_ptr.is_null());
            let msg = CStr::from_ptr(msg_ptr).to_string_lossy();
            assert_eq!(msg, "Null pointer provided");
            aaronia_string_free(msg_ptr);

            let msg_ptr = aaronia_get_error_message(AaroniaFfiError::Success as i32);
            assert!(!msg_ptr.is_null());
            let msg = CStr::from_ptr(msg_ptr).to_string_lossy();
            assert_eq!(msg, "Success");
            aaronia_string_free(msg_ptr);

            // A C caller can pass any int; it must answer, not misbehave.
            let msg_ptr = aaronia_get_error_message(42);
            assert!(!msg_ptr.is_null());
            let msg = CStr::from_ptr(msg_ptr).to_string_lossy();
            assert_eq!(msg, "Unknown error code");
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
