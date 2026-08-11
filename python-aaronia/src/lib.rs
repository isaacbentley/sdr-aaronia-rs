#![allow(unsafe_op_in_unsafe_fn)]
#![allow(unexpected_cfgs)]
#![allow(clippy::useless_conversion)]
use arrow::array::{Array, FixedSizeListArray, Float32Array};
use arrow::datatypes::{DataType, Field};
use arrow::pyarrow::ToPyArrow;
use num_complex::Complex32;
use numpy::PyArray1;
use pyo3::create_exception;
use pyo3::prelude::*;
use sdr_aaronia_rs::http_streaming::StreamFormat;
use sdr_aaronia_rs::{AaroniaConfig, AaroniaSource};
use std::sync::Arc;
use tokio::runtime::Runtime;

create_exception!(
    aaronia,
    AaroniaConnectionError,
    pyo3::exceptions::PyException
);
create_exception!(aaronia, AaroniaHardwareError, pyo3::exceptions::PyException);
create_exception!(aaronia, AaroniaTimeoutError, pyo3::exceptions::PyException);

// Helper to map Rust errors to Python exceptions
fn map_err<E: std::fmt::Display>(e: E) -> PyErr {
    let msg = e.to_string();
    if msg.contains("timeout") || msg.contains("Timeout") {
        AaroniaTimeoutError::new_err(msg)
    } else if msg.contains("Connection") || msg.contains("HTTP") {
        AaroniaConnectionError::new_err(msg)
    } else {
        AaroniaHardwareError::new_err(msg)
    }
}

#[pyclass(name = "AaroniaConfig")]
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

    #[setter]
    fn set_format(&mut self, format: &str) {
        self.inner.stream_format = match format {
            "F32" => Some(StreamFormat::Float32),
            "F16" => Some(StreamFormat::Float16),
            "I16" => Some(StreamFormat::Int16),
            _ => Some(StreamFormat::Float32),
        };
    }

    #[setter]
    fn set_center_freq(&mut self, freq: f64) {
        self.inner.center_frequency = freq;
    }

    #[setter]
    fn set_sample_rate(&mut self, rate: f64) {
        self.inner.span_frequency = rate;
    }
}

#[pyclass(name = "AaroniaSource")]
struct PyAaroniaSource {
    source: Option<AaroniaSource>,
    rt: Arc<Runtime>,
    buffer: Vec<Complex32>,
}

#[pymethods]
impl PyAaroniaSource {
    #[new]
    fn new() -> PyResult<Self> {
        let rt = Runtime::new().map_err(map_err)?;
        Ok(Self {
            source: None,
            rt: Arc::new(rt),
            buffer: Vec::new(),
        })
    }

    fn start_streaming(&mut self, config: &PyAaroniaConfig) -> PyResult<()> {
        let rt = self.rt.clone();
        let config_inner = config.inner.clone();
        let mut source = rt
            .block_on(async { AaroniaSource::new(config_inner).await })
            .map_err(map_err)?;

        rt.block_on(async { source.start_streaming().await })
            .map_err(map_err)?;

        self.source = Some(source);
        Ok(())
    }

    fn stop_streaming(&mut self) -> PyResult<()> {
        if let Some(mut source) = self.source.take() {
            self.rt
                .block_on(async { source.stop_streaming().await })
                .map_err(map_err)?;
        }
        Ok(())
    }

    fn read_samples_numpy<'py>(
        &mut self,
        py: Python<'py>,
        count: usize,
    ) -> PyResult<Bound<'py, PyArray1<Complex32>>> {
        let source = self
            .source
            .as_mut()
            .ok_or_else(|| AaroniaHardwareError::new_err("Not streaming"))?;

        if self.buffer.len() < count {
            self.buffer.resize(count, Complex32::new(0.0, 0.0));
        }

        let read = self
            .rt
            .block_on(async { source.read_samples(&mut self.buffer, count).await })
            .map_err(map_err)?;

        Ok(numpy::PyArray1::from_slice_bound(py, &self.buffer[..read]))
    }

    fn read_samples_arrow<'py>(&mut self, py: Python<'py>, count: usize) -> PyResult<PyObject> {
        let source = self
            .source
            .as_mut()
            .ok_or_else(|| AaroniaHardwareError::new_err("Not streaming"))?;

        if self.buffer.len() < count {
            self.buffer.resize(count, Complex32::new(0.0, 0.0));
        }

        let read = self
            .rt
            .block_on(async { source.read_samples(&mut self.buffer, count).await })
            .map_err(map_err)?;

        let mut float_values = Vec::with_capacity(read * 2);
        for c in self.buffer.iter().take(read) {
            float_values.push(c.re);
            float_values.push(c.im);
        }

        let values_array = Float32Array::from(float_values);
        let list_array = FixedSizeListArray::try_new(
            Arc::new(Field::new("item", DataType::Float32, false)),
            2,
            Arc::new(values_array),
            None,
        )
        .map_err(map_err)?;

        list_array.to_data().to_pyarrow(py)
    }

    fn take_overrun(&mut self) -> PyResult<bool> {
        let source = self
            .source
            .as_mut()
            .ok_or_else(|| AaroniaHardwareError::new_err("Not streaming"))?;
        Ok(source.take_overrun())
    }

    fn cumulative_drops(&self) -> PyResult<u64> {
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| AaroniaHardwareError::new_err("Not streaming"))?;
        Ok(source.cumulative_drops())
    }

    fn last_timestamp_ns(&self) -> PyResult<i64> {
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| AaroniaHardwareError::new_err("Not streaming"))?;
        Ok(source.last_timestamp_ns())
    }
}

#[pymodule]
fn aaronia(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    pyo3_log::init();

    m.add_class::<PyAaroniaConfig>()?;
    m.add_class::<PyAaroniaSource>()?;

    m.add(
        "AaroniaConnectionError",
        py.get_type_bound::<AaroniaConnectionError>(),
    )?;
    m.add(
        "AaroniaHardwareError",
        py.get_type_bound::<AaroniaHardwareError>(),
    )?;
    m.add(
        "AaroniaTimeoutError",
        py.get_type_bound::<AaroniaTimeoutError>(),
    )?;

    Ok(())
}
