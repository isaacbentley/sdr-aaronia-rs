use crate::Result;
use futuresdr::prelude::*;
use num_complex::Complex32;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

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
    /// Zero is clamped to one: an empty buffer can never fill, and
    /// `work` would spin on it forever.
    pub fn buffer_size(mut self, size: usize) -> Self {
        self.buffer_size = size.max(1);
        self
    }

    /// Set the streaming delay in seconds. This allows queuing of transmission requests ahead of time to accommodate jitter.
    #[must_use]
    pub fn streaming_delay(mut self, delay_s: f64) -> Self {
        self.streaming_delay = delay_s;
        self
    }

    /// Set the per-push HTTP timeout, in milliseconds. Each transmit batch
    /// that exceeds this is abandoned and counted in
    /// [`HttpSink::dropped_samples`] instead of blocking the background
    /// sender (and back-pressuring the flowgraph).
    #[must_use]
    pub fn timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
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
    sample_rate: f64,
    buffer_size: usize,
    sample_buffer: Vec<Complex32>,
    last_transmission_end_time: f64,
    streaming_delay: f64,
    tx: tokio::sync::mpsc::Sender<(Vec<f32>, f64, f64)>, // (samples, start_time, end_time)
    dropped_samples: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Handle to the background task that drains `tx` and performs the
    /// HTTP pushes. Held so `Drop` can abort it explicitly rather than
    /// relying on the task noticing a dropped `tx` on its next `recv()`.
    sender_task: tokio::task::JoinHandle<()>,
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
        timeout_ms: u64,
        auth_method: AuthMethod,
        streaming_delay: f64,
    ) -> Result<Self> {
        let buffer_size = buffer_size.max(1);
        let endpoints_client = HttpEndpointsClient::new(base_url, auth_method)?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Vec<f32>, f64, f64)>(16);
        let dropped_samples = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let dropped_samples_clone = dropped_samples.clone();

        let handle = tokio::runtime::Handle::try_current().map_err(|_| {
            crate::Error::Io(std::io::Error::other(
                "HttpSink must be created within a Tokio runtime context",
            ))
        })?;

        let push_timeout = std::time::Duration::from_millis(timeout_ms);
        let sender_task = handle.spawn(async move {
            while let Some((samples, start_time, end_time)) = rx.recv().await {
                let num_complex = samples.len() / 2;
                let req = TxSampleRequest {
                    start_time,
                    end_time,
                    start_frequency: frequency - sample_rate / 2.0,
                    end_frequency: frequency + sample_rate / 2.0,
                    step_frequency: None,
                    min_power: -2.0,
                    max_power: 2.0,
                    sample_size: 2,
                    sample_depth: 1,
                    unit: "volt".to_string(),
                    payload: "iq".to_string(),
                    push: true,
                    samples: &samples,
                };

                // Bound each push so a stalled TX link can't wedge the sender
                // task (and back-pressure the flowgraph) indefinitely. A
                // timed-out or failed batch is counted as dropped.
                match tokio::time::timeout(push_timeout, endpoints_client.push_samples(&req)).await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        dropped_samples_clone
                            .fetch_add(num_complex as u64, std::sync::atomic::Ordering::Relaxed);
                        warn!("Failed to push {} samples to RTSA: {}", num_complex, e);
                    }
                    Err(_elapsed) => {
                        dropped_samples_clone
                            .fetch_add(num_complex as u64, std::sync::atomic::Ordering::Relaxed);
                        warn!(
                            "Timed out ({} ms) pushing {} samples to RTSA",
                            timeout_ms, num_complex
                        );
                    }
                }
            }
        });

        Ok(Self {
            sample_rate,
            buffer_size,
            sample_buffer: Vec::with_capacity(buffer_size * 2),
            last_transmission_end_time: now,
            streaming_delay,
            tx,
            dropped_samples,
            sender_task,
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
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    async fn push_batch(&mut self) -> Result<()> {
        if self.sample_buffer.is_empty() {
            return Ok(());
        }

        const _: () = assert!(std::mem::size_of::<Complex32>() == 2 * std::mem::size_of::<f32>());

        let num_complex = self.sample_buffer.len();
        let samples_slice: &[f32] = unsafe {
            std::slice::from_raw_parts(self.sample_buffer.as_ptr() as *const f32, num_complex * 2)
        };
        let samples_vec = samples_slice.to_vec();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let desired_start = now + self.streaming_delay;

        let start_time = desired_start.max(self.last_transmission_end_time);
        let duration = num_complex as f64 / self.sample_rate;
        let end_time = start_time + duration;

        self.last_transmission_end_time = end_time + (1.0 / self.sample_rate);

        if self
            .tx
            .send((samples_vec, start_time, end_time))
            .await
            .is_err()
        {
            // The background sender task is gone (e.g. it panicked), so
            // this batch will never reach the device. Count it the same
            // way a failed/timed-out push is counted — otherwise
            // `dropped_samples()`, documented as "the way to detect a
            // persistently broken link", would silently miss this failure
            // mode entirely.
            self.dropped_samples
                .fetch_add(num_complex as u64, std::sync::atomic::Ordering::Relaxed);
            warn!(
                "Failed to send {} samples to background HTTP task (channel closed)",
                num_complex
            );
        }

        self.sample_buffer.clear();
        Ok(())
    }
}

impl Drop for HttpSink {
    fn drop(&mut self) {
        // Explicit abort rather than relying on `tx`'s drop to close the
        // channel and have the task notice on its next `recv()` — mirrors
        // `AaroniaSource`'s `http_task` handling for the same reason: a
        // background task should not linger past its owner's lifetime.
        self.sender_task.abort();
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
            // Finish *now*, in this call. The previous code left this
            // branch empty ("next call will handle io.finished"), which
            // silently assumed the runtime would invoke work() once more
            // after the upstream's finish notification. When the finish
            // flag arrived together with the final samples — the normal
            // case for a short VectorSource graph — no further callback
            // is guaranteed, and the flowgraph hung forever awaiting a
            // work() call that never came. That is the intermittent
            // wedge that stalled a windows-latest CI runner for ~2 hours
            // and reproduced under the constrained Linux VM: timing
            // determined whether an extra callback happened to arrive.
            if !self.sample_buffer.is_empty() {
                let _ = self.push_batch().await;
            }
            io.finished = true;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: if the background sender task is gone (aborted or
    /// panicked), `push_batch` must still count the batch as dropped
    /// instead of silently discarding it — otherwise `dropped_samples()`,
    /// documented as "the way to detect a persistently broken link", would
    /// miss this failure mode entirely.
    #[tokio::test]
    async fn push_batch_counts_drop_when_sender_task_gone() {
        let mut sink = HttpSinkBuilder::new("http://localhost:54664")
            .buffer_size(4)
            .build()
            .expect("HttpSink should build inside a tokio runtime");

        // Kill the background sender task and give the abort a moment to
        // take effect, so the channel is genuinely closed before we push.
        sink.sender_task.abort();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(sink.dropped_samples(), 0);

        // Populate the private sample buffer directly (in-module access)
        // rather than driving it through a full flowgraph.
        sink.sample_buffer = vec![Complex32::new(1.0, -1.0); 3];
        sink.push_batch()
            .await
            .expect("push_batch itself must not error on a closed channel");

        assert_eq!(
            sink.dropped_samples(),
            3,
            "closed-channel send failure must count as dropped samples"
        );
        assert!(
            sink.sample_buffer.is_empty(),
            "buffer must still be cleared after the failed push"
        );
    }
}
