//! Python bindings for `sdr-aaronia-rs`.
//!
//! Exposes [`AaroniaConfig`]/[`AaroniaSource`] as the `aaronia` Python
//! module. Sample reads come back as NumPy arrays or PyArrow
//! `FixedSizeListArray`s of `[re, im]` float32 pairs; both paths copy
//! the samples out of the Rust receive buffer exactly once (an earlier
//! revision advertised "zero-copy", which was never true — and worse,
//! misused the core `read_samples` append contract so both APIs
//! returned freshly-zeroed memory instead of samples, growing the
//! scratch buffer without bound. See `read_scratch` below.)
//!
//! All blocking work (connect, reads with their `read_timeout`, default
//! 30 s, shutdown) releases the GIL, so other Python threads keep
//! running and `KeyboardInterrupt` stays deliverable between calls.

use arrow::array::{Array, FixedSizeListArray};
use arrow::datatypes::{DataType, Field};
use arrow::pyarrow::ToPyArrow;
use num_complex::Complex32;
use numpy::PyArray1;
use pyo3::create_exception;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use sdr_aaronia_rs::http_streaming::StreamFormat;
use sdr_aaronia_rs::{AaroniaConfig, AaroniaSource, Error as AaroniaError};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;

create_exception!(
    aaronia,
    AaroniaConnectionError,
    pyo3::exceptions::PyException,
    "The RTSA HTTP endpoint or device could not be reached."
);
create_exception!(
    aaronia,
    AaroniaHardwareError,
    pyo3::exceptions::PyException,
    "The device/SDK reported an error."
);
create_exception!(
    aaronia,
    AaroniaTimeoutError,
    pyo3::exceptions::PyException,
    "A read or control operation timed out."
);
create_exception!(
    aaronia,
    AaroniaStreamClosed,
    AaroniaConnectionError,
    "The sample stream ended and will produce no more data."
);

/// Largest single read request, in complex samples (512 MiB of
/// buffer). Bounds the up-front allocation so a typo'd `count` raises
/// `ValueError` instead of aborting the interpreter on allocation
/// failure.
const MAX_READ_SAMPLES: usize = 1 << 26;

/// Render an error with its `source()` chain — `Display` alone drops
/// the underlying cause (e.g. the connection-refused inside a reqwest
/// transport error), which made every failure look alike from Python.
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut msg = e.to_string();
    let mut cur = e.source();
    while let Some(src) = cur {
        msg.push_str(": ");
        msg.push_str(&src.to_string());
        cur = src.source();
    }
    msg
}

/// Map the crate's structured error to a typed Python exception by
/// *variant*, not by substring-matching display strings (the earlier
/// approach misrouted lowercase "operation timed out" and every
/// `Config` error to `AaroniaHardwareError`).
fn map_aaronia_err(e: AaroniaError) -> PyErr {
    let msg = error_chain(&e);
    match &e {
        AaroniaError::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => {
            AaroniaTimeoutError::new_err(msg)
        }
        AaroniaError::Transport(t) if t.is_timeout() => AaroniaTimeoutError::new_err(msg),
        // Subclasses AaroniaConnectionError, so `except
        // AaroniaConnectionError` still catches it — but a loop over
        // blocks can tell "no more data" from "a read failed".
        AaroniaError::StreamClosed(_) => AaroniaStreamClosed::new_err(msg),
        AaroniaError::Http { .. } | AaroniaError::Transport(_) | AaroniaError::Protocol(_) => {
            AaroniaConnectionError::new_err(msg)
        }
        AaroniaError::Config(_) | AaroniaError::Initialization(_) => PyValueError::new_err(msg),
        _ => AaroniaHardwareError::new_err(msg),
    }
}

/// Fallback for non-`Error` failure types (runtime construction,
/// arrow layout errors).
fn map_any_err<E: std::fmt::Display>(e: E) -> PyErr {
    AaroniaHardwareError::new_err(e.to_string())
}

/// Configuration for an [`AaroniaSource`].
///
/// Every field is settable *and* readable (the earlier revision was
/// write-only, and offered no way to reach a non-localhost device or a
/// recorded file at all).
#[pyclass(name = "AaroniaConfig", skip_from_py_object)]
#[derive(Clone)]
struct PyAaroniaConfig {
    inner: AaroniaConfig,
}

#[pymethods]
impl PyAaroniaConfig {
    #[new]
    fn new() -> Self {
        Self {
            inner: AaroniaConfig::default(),
        }
    }

    /// HTTP wire format: "F32", "F16", or "I16". Unknown values raise
    /// `ValueError` instead of silently defaulting.
    #[setter]
    fn set_format(&mut self, format: &str) -> PyResult<()> {
        self.inner.stream_format = match format {
            "F32" => Some(StreamFormat::Float32),
            "F16" => Some(StreamFormat::Float16),
            "I16" => Some(StreamFormat::Int16),
            other => {
                return Err(PyValueError::new_err(format!(
                    "unknown stream format {other:?}; expected \"F32\", \"F16\", or \"I16\""
                )));
            }
        };
        Ok(())
    }

    #[getter]
    fn get_format(&self) -> Option<&'static str> {
        self.inner.stream_format.map(|f| match f {
            StreamFormat::Float16 => "F16",
            StreamFormat::Int16 => "I16",
            _ => "F32",
        })
    }

    #[setter]
    fn set_center_freq(&mut self, freq: f64) {
        self.inner.center_frequency = freq;
    }

    #[getter]
    fn get_center_freq(&self) -> f64 {
        self.inner.center_frequency
    }

    /// IQ sample rate in Hz (the Aaronia "span" frequency).
    #[setter]
    fn set_sample_rate(&mut self, rate: f64) {
        self.inner.span_frequency = rate;
    }

    #[getter]
    fn get_sample_rate(&self) -> f64 {
        self.inner.span_frequency
    }

    #[setter]
    fn set_reference_level(&mut self, dbm: f64) {
        self.inner.reference_level = dbm;
    }

    #[getter]
    fn get_reference_level(&self) -> f64 {
        self.inner.reference_level
    }

    /// Base URL of an RTSA-Suite HTTP server block
    /// (e.g. `"http://atc.local:54664"`). Setting this pins the source
    /// to the HTTP backend.
    #[setter]
    fn set_http_base_url(&mut self, url: Option<String>) {
        self.inner.http_base_url = url.clone();
        if url.is_some() {
            self.inner.force_source_type = Some(sdr_aaronia_rs::unified_source::SourceType::Http);
        }
    }

    #[getter]
    fn get_http_base_url(&self) -> Option<String> {
        self.inner.http_base_url.clone()
    }

    /// Path to a recorded `.rtsa` file. Setting this pins the source to
    /// the file backend.
    #[setter]
    fn set_file_path(&mut self, path: Option<String>) {
        self.inner.file_path = path.clone();
        if path.is_some() {
            self.inner.force_source_type = Some(sdr_aaronia_rs::unified_source::SourceType::File);
        }
    }

    #[getter]
    fn get_file_path(&self) -> Option<String> {
        self.inner.file_path.clone()
    }

    /// RX channel selection for native-SDK captures: "Rx1" (default),
    /// "Rx2", or "Rx1And2" (dual-channel; read with
    /// `read_samples_dual_numpy`).
    #[setter]
    fn set_receiver_channel(&mut self, channel: &str) -> PyResult<()> {
        use sdr_aaronia_rs::RxChannel;
        self.inner.receiver_channel = Some(match channel {
            "Rx1" => RxChannel::Rx1,
            "Rx2" => RxChannel::Rx2,
            "Rx1And2" => RxChannel::Rx1And2,
            other => {
                return Err(PyValueError::new_err(format!(
                    "unknown receiver channel {other:?}; expected \"Rx1\", \"Rx2\", or \"Rx1And2\""
                )));
            }
        });
        Ok(())
    }

    #[getter]
    fn get_receiver_channel(&self) -> Option<&'static str> {
        use sdr_aaronia_rs::RxChannel;
        self.inner.receiver_channel.map(|c| match c {
            RxChannel::Rx2 => "Rx2",
            RxChannel::Rx1And2 => "Rx1And2",
            _ => "Rx1",
        })
    }

    #[setter]
    fn set_device_serial(&mut self, serial: Option<String>) {
        self.inner.device_serial = serial;
    }

    #[getter]
    fn get_device_serial(&self) -> Option<String> {
        self.inner.device_serial.clone()
    }

    /// Seconds a blocking read waits for samples before raising
    /// `AaroniaTimeoutError` (default 30.0). Must be > 0.
    #[setter]
    fn set_read_timeout(&mut self, seconds: f64) -> PyResult<()> {
        // `try_from_secs_f64`, not `from_secs_f64`: the latter panics on
        // NaN/negative/overflowing input, which would abort the whole
        // interpreter over a bad assignment.
        let timeout = Duration::try_from_secs_f64(seconds).map_err(|e| {
            PyValueError::new_err(format!("read_timeout is not a valid duration: {e}"))
        })?;
        if timeout.is_zero() {
            return Err(PyValueError::new_err(
                "read_timeout must be greater than zero seconds",
            ));
        }
        self.inner.read_timeout = timeout;
        Ok(())
    }

    #[getter]
    fn get_read_timeout(&self) -> f64 {
        self.inner.read_timeout.as_secs_f64()
    }

    /// Reconnect the HTTP stream automatically when the server closes it
    /// or the transport fails (default `True`). The first read after a
    /// gap reports an overrun.
    #[setter]
    fn set_auto_reconnect(&mut self, enabled: bool) {
        self.inner.auto_reconnect = enabled;
    }

    #[getter]
    fn get_auto_reconnect(&self) -> bool {
        self.inner.auto_reconnect
    }
}

/// The `(rx1, rx2)` pair returned by `read_samples_dual_numpy`.
type DualArrays<'py> = (
    Bound<'py, PyArray1<Complex32>>,
    Bound<'py, PyArray1<Complex32>>,
);

/// A streaming IQ source. Construct, `start_streaming(config)`, then
/// call the `read_samples_*` methods; each blocks (GIL released) until
/// `count` samples arrive or `config.read_timeout` (default 30 s)
/// elapses, which raises `AaroniaTimeoutError`.
#[pyclass(name = "AaroniaSource")]
struct PyAaroniaSource {
    // Field order matters: `source` must drop before `rt` so the
    // source's background tasks shut down while the runtime is alive.
    source: Option<AaroniaSource>,
    rt: Arc<Runtime>,
    /// Scratch buffer reused across reads. `read_samples` *appends* to
    /// the Vec it is given, so every read starts with `clear()` —
    /// capacity is retained, no per-read allocation in steady state.
    read_scratch: Vec<Complex32>,
}

impl PyAaroniaSource {
    /// Clear the scratch, read up to `count` samples (GIL released),
    /// and return how many landed in `self.read_scratch[..n]`.
    fn read_into_scratch(&mut self, py: Python<'_>, count: usize) -> PyResult<usize> {
        if count > MAX_READ_SAMPLES {
            return Err(PyValueError::new_err(format!(
                "count {count} exceeds the per-read limit of {MAX_READ_SAMPLES} samples"
            )));
        }
        let source = self
            .source
            .as_mut()
            .ok_or_else(|| AaroniaHardwareError::new_err("Not streaming"))?;

        self.read_scratch.clear();
        let rt = self.rt.clone();
        let scratch = &mut self.read_scratch;
        py.detach(|| rt.block_on(source.read_samples(scratch, count)))
            .map_err(map_aaronia_err)
    }
}

#[pymethods]
impl PyAaroniaSource {
    #[new]
    fn new() -> PyResult<Self> {
        let rt = Runtime::new().map_err(map_any_err)?;
        Ok(Self {
            source: None,
            rt: Arc::new(rt),
            read_scratch: Vec::new(),
        })
    }

    /// Connect to the backend selected by `config` and start streaming.
    /// Blocking (GIL released).
    fn start_streaming(&mut self, py: Python<'_>, config: &PyAaroniaConfig) -> PyResult<()> {
        let rt = self.rt.clone();
        let config_inner = config.inner.clone();
        let source = py
            .detach(|| {
                rt.block_on(async {
                    let mut source = AaroniaSource::new(config_inner).await?;
                    source.start_streaming().await?;
                    Ok::<_, AaroniaError>(source)
                })
            })
            .map_err(map_aaronia_err)?;

        self.source = Some(source);
        Ok(())
    }

    /// Stop streaming and release the backend. Blocking (GIL released).
    fn stop_streaming(&mut self, py: Python<'_>) -> PyResult<()> {
        if let Some(mut source) = self.source.take() {
            let rt = self.rt.clone();
            py.detach(|| rt.block_on(source.stop_streaming()))
                .map_err(map_aaronia_err)?;
        }
        Ok(())
    }

    /// Read up to `count` IQ samples as a NumPy `complex64` array.
    /// The samples are copied once into a NumPy-owned buffer (safe to
    /// hold after further reads or after the source is dropped).
    fn read_samples_numpy<'py>(
        &mut self,
        py: Python<'py>,
        count: usize,
    ) -> PyResult<Bound<'py, PyArray1<Complex32>>> {
        let read = self.read_into_scratch(py, count)?;
        Ok(PyArray1::from_slice(py, &self.read_scratch[..read]))
    }

    /// Read up to `count` IQ samples as a PyArrow `FixedSizeListArray`
    /// of `[re, im]` float32 pairs. The samples are copied once while
    /// building the Arrow buffer; the Arrow → pyarrow handoff itself is
    /// the standard C-Data interface (no further copy).
    fn read_samples_arrow<'py>(
        &mut self,
        py: Python<'py>,
        count: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let read = self.read_into_scratch(py, count)?;

        let mut float_values = Vec::with_capacity(read * 2);
        for c in self.read_scratch.iter().take(read) {
            float_values.push(c.re);
            float_values.push(c.im);
        }

        let list_array = FixedSizeListArray::try_new(
            Arc::new(Field::new("item", DataType::Float32, false)),
            2,
            Arc::new(arrow::array::Float32Array::from(float_values)),
            None,
        )
        .map_err(map_any_err)?;

        list_array.to_data().to_pyarrow(py)
    }

    /// Retune the running source's center frequency in Hz. Blocking
    /// (GIL released). Previously the only way to retune was tearing
    /// down and rebuilding the whole source.
    fn set_center_frequency(&mut self, py: Python<'_>, freq_hz: f64) -> PyResult<()> {
        let source = self
            .source
            .as_mut()
            .ok_or_else(|| AaroniaHardwareError::new_err("Not streaming"))?;
        let rt = self.rt.clone();
        py.detach(|| rt.block_on(source.set_center_frequency(freq_hz)))
            .map_err(map_aaronia_err)
    }

    /// Change the running source's IQ sample rate in Hz (validated
    /// against the IQ-mode constraint). Blocking (GIL released).
    fn set_sample_rate(&mut self, py: Python<'_>, rate_hz: f64) -> PyResult<()> {
        let source = self
            .source
            .as_mut()
            .ok_or_else(|| AaroniaHardwareError::new_err("Not streaming"))?;
        let rt = self.rt.clone();
        py.detach(|| rt.block_on(source.set_span_frequency(rate_hz)))
            .map_err(map_aaronia_err)
    }

    /// Change the running source's reference level in dBm. Blocking
    /// (GIL released).
    fn set_reference_level(&mut self, py: Python<'_>, dbm: f64) -> PyResult<()> {
        let source = self
            .source
            .as_mut()
            .ok_or_else(|| AaroniaHardwareError::new_err("Not streaming"))?;
        let rt = self.rt.clone();
        py.detach(|| rt.block_on(source.set_reference_level(dbm)))
            .map_err(map_aaronia_err)
    }

    /// Read up to `count` time-aligned (Rx1, Rx2) sample pairs from a
    /// dual-channel capture as two NumPy `complex64` arrays of equal
    /// length. Requires `config.receiver_channel = "Rx1And2"` on the
    /// native-SDK backend.
    fn read_samples_dual_numpy<'py>(
        &mut self,
        py: Python<'py>,
        count: usize,
    ) -> PyResult<DualArrays<'py>> {
        if count > MAX_READ_SAMPLES {
            return Err(PyValueError::new_err(format!(
                "count {count} exceeds the per-read limit of {MAX_READ_SAMPLES} samples"
            )));
        }
        let source = self
            .source
            .as_mut()
            .ok_or_else(|| AaroniaHardwareError::new_err("Not streaming"))?;
        let rt = self.rt.clone();
        let (rx1, rx2) = py
            .detach(|| {
                rt.block_on(async {
                    let mut rx1 = Vec::new();
                    let mut rx2 = Vec::new();
                    source.read_samples_dual(&mut rx1, &mut rx2, count).await?;
                    Ok::<_, AaroniaError>((rx1, rx2))
                })
            })
            .map_err(map_aaronia_err)?;
        Ok((
            PyArray1::from_slice(py, &rx1),
            PyArray1::from_slice(py, &rx2),
        ))
    }

    /// True once if a receive-side overrun (dropped packets) occurred
    /// since the last call.
    fn take_overrun(&mut self) -> PyResult<bool> {
        let source = self
            .source
            .as_mut()
            .ok_or_else(|| AaroniaHardwareError::new_err("Not streaming"))?;
        Ok(source.take_overrun())
    }

    /// Enter a `with` block. The source is already streaming.
    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    /// Leave a `with` block, stopping the stream even when the body
    /// raised.
    ///
    /// Never suppresses the body's exception. A failure to stop is
    /// raised only when the body itself succeeded — otherwise it would
    /// replace the error that actually explains what went wrong.
    #[pyo3(signature = (exc_type=None, _exc_value=None, _traceback=None))]
    fn __exit__(
        &mut self,
        py: Python<'_>,
        exc_type: Option<Py<PyAny>>,
        _exc_value: Option<Py<PyAny>>,
        _traceback: Option<Py<PyAny>>,
    ) -> PyResult<bool> {
        let stopped = self.stop_streaming(py);
        if exc_type.is_none() {
            stopped?;
        }
        Ok(false)
    }

    /// Iterate fixed-size blocks of samples.
    ///
    /// Ends cleanly when the stream closes, rather than raising, so a
    /// `for` loop terminates the way a reader expects.
    fn blocks(slf: Py<Self>, count: usize) -> BlockIterator {
        BlockIterator { source: slf, count }
    }

    /// Total dropped samples reported by the drop detector.
    fn cumulative_drops(&self) -> PyResult<u64> {
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| AaroniaHardwareError::new_err("Not streaming"))?;
        Ok(source.cumulative_drops())
    }

    /// Timestamp (epoch ns) of the most recently received network
    /// block. HTTP backend only; 0 when unavailable.
    fn last_timestamp_ns(&self) -> PyResult<i64> {
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| AaroniaHardwareError::new_err("Not streaming"))?;
        Ok(source.last_timestamp_ns())
    }
}

/// Iterator over fixed-size sample blocks, returned by
/// [`AaroniaSource.blocks`].
#[pyclass(name = "BlockIterator")]
struct BlockIterator {
    source: Py<PyAaroniaSource>,
    count: usize,
}

#[pymethods]
impl BlockIterator {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        let mut source = self.source.borrow_mut(py);
        match source.read_samples_numpy(py, self.count) {
            // File playback signals its end with an empty read rather
            // than an error; without this the loop spins forever on
            // empty arrays once the recording runs out.
            Ok(arr) if arr.len()? == 0 => Ok(None),
            Ok(arr) => Ok(Some(arr.into_any().unbind())),
            // Only a finished stream ends the loop. Timeouts, transport
            // failures and protocol errors all propagate: a truncated
            // capture must not look like one that simply ran out.
            Err(e) if e.is_instance_of::<AaroniaStreamClosed>(py) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// The IQ sample rates the hardware can actually run, highest first.
///
/// Anything else is silently adjusted by the device, so a program that
/// keeps using the rate it asked for computes every derived frequency
/// against a rate that is not in use.
#[pyfunction]
fn sample_rates() -> Vec<f64> {
    sdr_aaronia_rs::iq_sample_rates().to_vec()
}

/// The lowest sample rate whose alias-free bandwidth covers
/// `bandwidth_hz`.
///
/// Wanting 8 MHz of spectrum needs 10 MHz of sampling, so this returns
/// 15.36 MHz, the lowest rate that provides it.
#[pyfunction]
fn sample_rate_for_bandwidth(bandwidth_hz: f64) -> f64 {
    sdr_aaronia_rs::iq_sample_rate_for_bandwidth(bandwidth_hz)
}

/// Open a source and start streaming, in one call.
///
/// Give either `rate` (an exact sample rate) or `bandwidth` (how much
/// spectrum you want to see, from which a real rate is chosen). Pass
/// `file` instead of `url` to play back a recording.
#[pyfunction]
#[pyo3(signature = (url=None, *, freq=None, rate=None, bandwidth=None, ref_level=None, file=None, format=None, read_timeout=None))]
#[allow(clippy::too_many_arguments)]
fn open(
    py: Python<'_>,
    url: Option<String>,
    freq: Option<f64>,
    rate: Option<f64>,
    bandwidth: Option<f64>,
    ref_level: Option<f64>,
    file: Option<String>,
    format: Option<&str>,
    read_timeout: Option<f64>,
) -> PyResult<Py<PyAaroniaSource>> {
    if url.is_some() && file.is_some() {
        return Err(PyValueError::new_err(
            "give url or file, not both: they select different backends",
        ));
    }
    if rate.is_some() && bandwidth.is_some() {
        return Err(PyValueError::new_err(
            "give rate or bandwidth, not both: bandwidth chooses a rate for you",
        ));
    }

    let mut cfg = PyAaroniaConfig::new();
    match (&url, &file) {
        (Some(u), _) => cfg.set_http_base_url(Some(u.clone())),
        (None, Some(f)) => cfg.set_file_path(Some(f.clone())),
        // Neither: the RTSA HTTP server's own default, which is where
        // it listens on the machine running RTSA-Suite.
        (None, None) => cfg.set_http_base_url(Some("http://localhost:54664".to_string())),
    }
    if let Some(freq) = freq {
        cfg.set_center_freq(freq);
    }
    if let Some(rate) = rate {
        cfg.set_sample_rate(rate);
    } else if let Some(bw) = bandwidth {
        cfg.set_sample_rate(sdr_aaronia_rs::iq_sample_rate_for_bandwidth(bw));
    }
    if let Some(dbm) = ref_level {
        cfg.set_reference_level(dbm);
    }
    if let Some(fmt) = format {
        cfg.set_format(fmt)?;
    }
    if let Some(seconds) = read_timeout {
        cfg.set_read_timeout(seconds)?;
    }

    let mut source = PyAaroniaSource::new()?;
    source.start_streaming(py, &cfg)?;
    Py::new(py, source)
}

/// How long [`diagnose`] may spend in total before giving up.
const DIAGNOSE_BUDGET: Duration = Duration::from_secs(20);

/// Check an RTSA-Suite HTTP server and report what is wrong.
///
/// Returns a list of `(ok, message, fix)` tuples. Every failure mode
/// here is one the quickstart documents; the point is that a program can
/// name the fix instead of a person rereading the guide after a timeout.
#[pyfunction]
#[pyo3(signature = (url="http://localhost:54664"))]
fn diagnose(py: Python<'_>, url: &str) -> PyResult<Vec<(bool, String, String)>> {
    use sdr_aaronia_rs::http_endpoints::{AuthMethod, HttpEndpointsClient};

    // A diagnostic makes a handful of small requests; a full
    // multi-threaded runtime is pure overhead for that.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(map_any_err)?;
    let url = url.to_string();

    py.detach(|| {
        rt.block_on(async move {
            let checks = async move {
                let mut out: Vec<(bool, String, String)> = Vec::new();

                let client = match HttpEndpointsClient::new(url.clone(), AuthMethod::None) {
                    Ok(c) => c,
                    Err(e) => {
                        out.push((
                            false,
                            format!("{url} is not a usable URL: {e}"),
                            "Pass a base URL such as http://localhost:54664".to_string(),
                        ));
                        return Ok(out);
                    }
                };

                match client.get_info().await {
                    Ok(info) => out.push((
                        true,
                        format!(
                            "reachable: {} on port {}, mission {:?}",
                            info.name, info.port, info.mission
                        ),
                        String::new(),
                    )),
                    Err(e) => {
                        out.push((
                            false,
                            format!("cannot reach {url}: {e}"),
                            "Start RTSA-Suite, add an HTTP Server block to the mission, and \
                         check the port. A *.local hostname can also take a moment to \
                         resolve from a cold start."
                                .to_string(),
                        ));
                        // Nothing else can be checked without a server.
                        return Ok(out);
                    }
                }

                let inputs = match client.get_inputs().await {
                    Ok(i) => i,
                    Err(e) => {
                        out.push((
                            false,
                            format!("cannot list inputs: {e}"),
                            "The server answered /info but not /inputs, which is unusual; \
                         check the RTSA-Suite version."
                                .to_string(),
                        ));
                        return Ok(out);
                    }
                };
                if inputs.is_empty() {
                    out.push((
                        false,
                        "the server has no inputs".to_string(),
                        "Connect the device block's output to the HTTP Server block's input \
                     in the mission."
                            .to_string(),
                    ));
                    return Ok(out);
                }
                out.push((
                    true,
                    format!("{} input(s): {}", inputs.len(), inputs.join(", ")),
                    String::new(),
                ));

                // Does anything actually carry IQ?
                let mut iq_input = None;
                let mut saw_any = false;
                for name in &inputs {
                    if let Ok(sample) = client.get_sample(Some(name)).await {
                        saw_any = true;
                        if sample.payload == sdr_aaronia_rs::http_streaming::PayloadType::Iq {
                            iq_input = Some((name.clone(), sample));
                            break;
                        }
                    }
                }
                match (&iq_input, saw_any) {
                    (Some((name, sample)), _) => {
                        let fs = sample.sample_frequency.unwrap_or(0.0);
                        let usable = sample.end_frequency - sample.start_frequency;
                        out.push((
                            true,
                            format!(
                                "input {name:?} carries IQ at {:.3} MHz sampling, \
                             {:.3} MHz usable, centred on {:.3} MHz",
                                fs / 1e6,
                                usable / 1e6,
                                (sample.start_frequency + sample.end_frequency) / 2.0 / 1e6
                            ),
                            String::new(),
                        ));

                        // A rate off the ladder means the device silently chose
                        // its own, which is worth surfacing. Compare
                        // proportionally: hardware reports its real clock, not
                        // the nominal figure — a healthy 15.36 MHz reads back
                        // as 15.359988 MHz — while a genuinely wrong rate is
                        // out by tens of percent.
                        if fs > 0.0 {
                            let nearest = sdr_aaronia_rs::nearest_iq_sample_rate(fs);
                            if ((nearest - fs) / nearest).abs() > 0.001 {
                                out.push((
                                    false,
                                    format!(
                                        "sampling at {:.6} MHz, which is not one of the \
                                     device's rates",
                                        fs / 1e6
                                    ),
                                    format!(
                                        "Ask for {:.3} MHz instead, or use \
                                     aaronia.sample_rate_for_bandwidth()",
                                        nearest / 1e6
                                    ),
                                ));
                            }
                        }
                    }
                    (None, true) => out.push((
                        false,
                        "no input carries IQ data".to_string(),
                        "Set the device block to IQ mode. A spectra payload means the \
                     mission is producing spectra, which the sample APIs do not read."
                            .to_string(),
                    )),
                    (None, false) => out.push((
                        false,
                        "inputs exist but none returned a sample".to_string(),
                        "The mission is probably not running, or the device block's output \
                     is not connected to the HTTP Server block."
                            .to_string(),
                    )),
                }

                Ok(out)
            };

            // Each request carries the client's own 30 s timeout, and a
            // stalled server would otherwise pay that per input. A
            // diagnostic that hangs is one nobody waits for.
            match tokio::time::timeout(DIAGNOSE_BUDGET, checks).await {
                Ok(result) => result,
                Err(_) => Ok(vec![(
                    false,
                    format!(
                        "checks did not finish within {} seconds",
                        DIAGNOSE_BUDGET.as_secs()
                    ),
                    "The server accepted the connection but stopped responding. \
                     Restart the RTSA-Suite mission."
                        .to_string(),
                )]),
            }
        })
    })
}

/// Console-script entry point: `aaronia-doctor [url]`.
#[pyfunction]
fn doctor_cli(py: Python<'_>) -> PyResult<i32> {
    let sys = py.import("sys")?;
    let argv: Vec<String> = sys.getattr("argv")?.extract()?;
    let url = argv
        .get(1)
        .cloned()
        .unwrap_or_else(|| "http://localhost:54664".to_string());

    println!("Checking {url}");
    let findings = diagnose(py, &url)?;
    let mut failed = false;
    for (ok, message, fix) in findings {
        if ok {
            println!("  ok    {message}");
        } else {
            failed = true;
            println!("  FAIL  {message}");
            if !fix.is_empty() {
                println!("        {fix}");
            }
        }
    }
    if failed {
        println!(
            "\nSee https://github.com/isaacbentley/sdr-aaronia-rs/blob/main/docs/QUICKSTART.md"
        );
    }
    Ok(i32::from(failed))
}

#[pymodule]
fn aaronia(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    // try_init: another Rust extension in the process may already have
    // installed a global logger; that must not turn `import aaronia`
    // into a PanicException.
    let _ = pyo3_log::try_init();

    m.add_class::<PyAaroniaConfig>()?;
    m.add_class::<PyAaroniaSource>()?;
    m.add_class::<BlockIterator>()?;
    m.add_function(wrap_pyfunction!(open, m)?)?;
    m.add_function(wrap_pyfunction!(sample_rates, m)?)?;
    m.add_function(wrap_pyfunction!(sample_rate_for_bandwidth, m)?)?;
    m.add_function(wrap_pyfunction!(diagnose, m)?)?;
    m.add_function(wrap_pyfunction!(doctor_cli, m)?)?;

    m.add(
        "AaroniaConnectionError",
        py.get_type::<AaroniaConnectionError>(),
    )?;
    m.add(
        "AaroniaHardwareError",
        py.get_type::<AaroniaHardwareError>(),
    )?;
    m.add("AaroniaTimeoutError", py.get_type::<AaroniaTimeoutError>())?;
    m.add("AaroniaStreamClosed", py.get_type::<AaroniaStreamClosed>())?;

    Ok(())
}
