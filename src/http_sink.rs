use crate::Result;
use futuresdr::prelude::*;
use num_complex::Complex32;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{trace, warn};

use crate::http_endpoints::{AuthMethod, HttpEndpointsClient, TxSampleRequest};

/// Builder for the `FutureSDR` [`HttpSink`] block.
pub struct HttpSinkBuilder {
    base_url: String,
    frequency: f64,
    sample_rate: f64,
    buffer_size: usize,
    timeout_ms: u64,
    auth_method: AuthMethod,
    streaming_delay: f64,
}

impl Default for HttpSinkBuilder {
    fn default() -> Self {
        Self::new("http://localhost:54664")
    }
}

impl HttpSinkBuilder {
    /// Create a new HttpSinkBuilder with a target base URL.
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.to_string(),
            frequency: 100e6,
            sample_rate: 1e6,
            buffer_size: 65536,
            timeout_ms: 15000,
            auth_method: AuthMethod::None,
            streaming_delay: 0.1, // Default 100ms
        }
    }

    /// Set the target transmission center frequency in Hz.
    #[must_use]
    pub fn frequency(mut self, freq: f64) -> Self {
        self.frequency = freq;
        self
    }

    /// Set the target transmission sample rate in Hz.
    #[must_use]
    pub fn sample_rate(mut self, rate: f64) -> Self {
        self.sample_rate = rate;
        self
    }

    /// Set the internal buffer size (number of samples per chunk sent via HTTP).
    #[must_use]
    pub fn buffer_size(mut self, size: usize) -> Self {
        self.buffer_size = size;
        self
    }

    /// Set the streaming delay in seconds. This allows queuing of transmission requests ahead of time to accommodate jitter.
    #[must_use]
    pub fn streaming_delay(mut self, delay_s: f64) -> Self {
        self.streaming_delay = delay_s;
        self
    }

    /// Set the authentication method for the endpoint.
    #[must_use]
    pub fn auth(mut self, auth: AuthMethod) -> Self {
        self.auth_method = auth;
        self
    }

    /// Build the FutureSDR Block.
    pub fn build(self) -> Result<HttpSink> {
        HttpSink::new(
            self.base_url,
            self.frequency,
            self.sample_rate,
            self.buffer_size,
            self.timeout_ms,
            self.auth_method,
            self.streaming_delay,
        )
    }
}

/// `FutureSDR` block for transmitting IQ samples to an Aaronia RTSA via HTTP.
#[derive(Block)]
pub struct HttpSink {
    endpoints_client: HttpEndpointsClient,
    frequency: f64,
    sample_rate: f64,
    buffer_size: usize,
    sample_buffer: Vec<Complex32>,
    last_transmission_end_time: f64,
    streaming_delay: f64,
    /// Total complex samples dropped so far because a push failed. Exposed
    /// via [`HttpSink::dropped_samples`] so a caller can detect a
    /// persistently failing TX link instead of it silently going quiet.
    dropped_samples: u64,
    #[input]
    input: futuresdr::runtime::buffer::DefaultCpuReader<Complex32>,
}

impl HttpSink {
    #[allow(clippy::too_many_arguments, clippy::new_ret_no_self)]
    pub fn new(
        base_url: String,
        frequency: f64,
        sample_rate: f64,
        buffer_size: usize,
        _timeout_ms: u64,
        auth_method: AuthMethod,
        streaming_delay: f64,
    ) -> Result<Self> {
        let endpoints_client = HttpEndpointsClient::new(base_url, auth_method)?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();

        Ok(Self {
            endpoints_client,
            frequency,
            sample_rate,
            buffer_size,
            sample_buffer: Vec::with_capacity(buffer_size * 2),
            last_transmission_end_time: now,
            streaming_delay,
            dropped_samples: 0,
            input: futuresdr::runtime::buffer::DefaultCpuReader::default(),
        })
    }

    /// Total complex samples dropped so far because a `push_samples` call
    /// failed. A steadily increasing count indicates the TX link is
    /// unhealthy — `work()` cannot fail the flowgraph on a transient push
    /// error (that would kill the whole graph over one dropped HTTP
    /// request), so this counter is the way to detect a persistently
    /// broken link instead of transmission silently going quiet.
    pub fn dropped_samples(&self) -> u64 {
        self.dropped_samples
    }

    async fn push_batch(&mut self) -> Result<()> {
        if self.sample_buffer.is_empty() {
            return Ok(());
        }

        // `Complex32` is `#[repr(C)] { re: f32, im: f32 }`, so it has
        // identical layout to `[f32; 2]`; this guards against a future
        // change to that representation breaking the raw-pointer cast
        // below.
        const _: () = assert!(std::mem::size_of::<Complex32>() == 2 * std::mem::size_of::<f32>());

        let num_complex = self.sample_buffer.len();
        let samples_slice: &[f32] = unsafe {
            std::slice::from_raw_parts(self.sample_buffer.as_ptr() as *const f32, num_complex * 2)
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let desired_start = now + self.streaming_delay;

        // Ensure we don't start before the previous burst ended to prevent overlaps
        let start_time = desired_start.max(self.last_transmission_end_time);

        // Duration of this burst in seconds
        let duration = num_complex as f64 / self.sample_rate;
        let end_time = start_time + duration;

        // Keep 1 sample spacing between consecutive requests
        self.last_transmission_end_time = end_time + (1.0 / self.sample_rate);

        let req = TxSampleRequest {
            start_time,
            end_time,
            start_frequency: self.frequency - self.sample_rate / 2.0,
            end_frequency: self.frequency + self.sample_rate / 2.0,
            step_frequency: None,
            min_power: -2.0,
            max_power: 2.0,
            sample_size: 2, // 2 elements for complex floats
            sample_depth: 1,
            unit: "volt".to_string(),
            payload: "iq".to_string(),
            push: true,
            samples: samples_slice,
        };

        match self.endpoints_client.push_samples(&req).await {
            Ok(_) => {
                trace!("Successfully pushed {} samples to RTSA", num_complex);
                self.sample_buffer.clear();
                Ok(())
            }
            Err(e) => {
                self.dropped_samples += num_complex as u64;
                warn!(
                    "Failed to push {} samples to RTSA (total dropped: {}): {}",
                    num_complex, self.dropped_samples, e
                );
                // Drop on failure to prevent unbounded stalling in the flowgraph stream.
                // `dropped_samples()` lets a caller detect a persistently
                // failing link instead of TX silently going quiet.
                self.sample_buffer.clear();
                Err(e)
            }
        }
    }
}

#[doc(hidden)]
impl Kernel for HttpSink {
    async fn work(
        &mut self,
        io: &mut futuresdr::runtime::WorkIo,
        _mio: &mut futuresdr::runtime::MessageOutputs,
        _meta: &mut futuresdr::runtime::BlockMeta,
    ) -> anyhow::Result<()> {
        let input_len = self.input.slice().len();

        if input_len == 0 {
            if self.input.finished() {
                // Flush remaining samples if the flowgraph is finishing
                if !self.sample_buffer.is_empty() {
                    let _ = self.push_batch().await;
                }
                io.finished = true;
                return Ok(());
            }
            return Ok(());
        }

        let mut consumed = 0;

        // Drain input into our buffer until buffer_size is reached
        while consumed < input_len {
            {
                let i = self.input.slice();
                let available_space = self.buffer_size.saturating_sub(self.sample_buffer.len());
                let to_take = available_space.min(i.len() - consumed);

                self.sample_buffer
                    .extend_from_slice(&i[consumed..consumed + to_take]);
                consumed += to_take;
            }

            if self.sample_buffer.len() >= self.buffer_size {
                // Buffer full, push a batch
                let _ = self.push_batch().await;
            }
        }

        self.input.consume(consumed);

        if self.input.finished() && consumed == input_len {
            // Next call will handle io.finished
        }

        Ok(())
    }
}
