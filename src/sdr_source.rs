//! Core SDR source traits and types.
//!
//! This module provides the standard `SdrSource` interface previously
//! maintained in `sdr-source-rs`, now integrated directly to allow for
//! a zero-dependency architecture.

use num_complex::Complex32;
use std::ops::Deref;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Helper to hash center frequencies to integer kHz boundaries.
pub fn freq_key_khz(freq_hz: f64) -> u64 {
    (freq_hz / 1000.0).round() as u64
}

/// A zero-copy buffer of IQ samples backed by a crossbeam-channel pool.
/// Returns to the pool automatically on Drop.
pub struct PooledIqBuffer {
    buffer: Option<Vec<Complex32>>,
    pool: Option<crossbeam_channel::Sender<Vec<Complex32>>>,
}

impl PooledIqBuffer {
    pub fn new_pooled(
        buffer: Vec<Complex32>,
        pool: crossbeam_channel::Sender<Vec<Complex32>>,
    ) -> Self {
        Self {
            buffer: Some(buffer),
            pool: Some(pool),
        }
    }
}

impl Deref for PooledIqBuffer {
    type Target = [Complex32];
    fn deref(&self) -> &Self::Target {
        self.buffer.as_deref().unwrap_or(&[])
    }
}

impl Drop for PooledIqBuffer {
    fn drop(&mut self) {
        if let Some(mut buf) = self.buffer.take()
            && let Some(pool) = &self.pool
        {
            buf.clear();
            let _ = pool.send(buf);
        }
    }
}

/// Standardized crossbeam message carrying IQ samples and metadata.
pub struct IqPacket {
    pub samples: PooledIqBuffer,
    pub center_frequency_hz: f64,
    pub sample_rate_hz: f32,
    pub overrun: bool,
}

/// Orchestrator-provided hints to drive adaptive dwell logic.
pub trait DwellAdvice: Send + Sync {
    fn latest_signal_at(&self, freq_key: u64) -> Option<Instant>;
}

/// Core configuration for an SDR capture session.
#[derive(Debug, Clone, Default)]
pub struct SourceConfig {
    pub sample_rate_hz: f64,
    pub channels_hz: Vec<f64>,
    pub dwell_min: Duration,
    pub dwell_max: Duration,
    pub dwell_extension: Duration,
}

/// Logic for extending or cutting short the dwell on a particular frequency
/// based on the orchestrator's `DwellAdvice`.
pub struct DwellController {
    pub min: Duration,
    pub max: Duration,
    pub extension: Duration,
}

impl DwellController {
    pub fn deadline(&self, hop_start: Instant, latest_signal: Option<Instant>) -> Instant {
        let base = hop_start + self.min;
        match latest_signal {
            Some(sig) if self.extension > Duration::ZERO => {
                let extended = sig + self.extension;
                let max_deadline = hop_start + self.max;
                base.max(extended).min(max_deadline)
            }
            _ => base,
        }
    }

    pub fn is_adaptive(&self) -> bool {
        self.extension > Duration::ZERO && self.max > self.min
    }
}

pub type SdrError = anyhow::Error;

/// Handle returned by an active `SdrSource` to stream packets and control the backend.
pub struct SdrHandle {
    pub receiver: crossbeam_channel::Receiver<IqPacket>,
    pub stop: Box<dyn Fn() + Send + Sync>,
    pub wait: Box<dyn FnOnce() + Send>,
}

/// The core entry point for native SDR integrations.
pub trait SdrSource {
    fn start(
        self: Box<Self>,
        config: SourceConfig,
        advice: Arc<dyn DwellAdvice>,
    ) -> Result<SdrHandle, SdrError>;
}
