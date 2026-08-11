//! Native [`seify`] driver implementation for `sdr-aaronia-rs`.
//!
//! Integration status: construct via [`AaroniaSeifyDevice::from_args`]
//! and use the trait objects directly (or through
//! `seify::dev::DynDeviceBackend`). The device is `Clone` (all state is
//! behind `Arc`s) as seify's `Device::from_impl` requires. It is *not*
//! part of seify's built-in enumeration registry — `seify::enumerate()`
//! will not discover it; open it explicitly.
//!
//! Blocking: every control call and `read` uses `Runtime::block_on`
//! internally and must not be called from within an async runtime.

use crate::unified_source::{AaroniaSource, AaroniaSourceBuilder, SourceType};
use num_complex::Complex32;
use seify::dev::DynDeviceBackend;
use seify::{
    Args, DeviceInfo, Direction, FrequencyControl, GainControl, Range, RangeItem, RxDevice,
    RxStreamer, SampleRateControl,
};
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;

/// Cached tuning state shared across clones; updated by the setters so
/// the getters reflect the last applied values (plain fields on the
/// device made every getter permanently stale after the first retune).
#[derive(Debug, Clone, Copy)]
struct Tuning {
    center_frequency: f64,
    sample_rate: f64,
    reference_level: f64,
}

/// Seify device wrapper around [`AaroniaSource`]. `Clone` shares the
/// underlying source/runtime/tuning (required by seify's
/// `Device::from_impl`).
#[derive(Clone)]
pub struct AaroniaSeifyDevice {
    source: Arc<Mutex<AaroniaSource>>,
    runtime: Arc<Runtime>,
    tuning: Arc<Mutex<Tuning>>,
}

impl AaroniaSeifyDevice {
    /// Create a new Seify device from arguments string.
    pub fn from_args(args: &Args) -> std::result::Result<Self, seify::Error> {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| seify::Error::Io(std::io::Error::other(e.to_string())))?,
        );

        let mut builder = AaroniaSourceBuilder::new();

        if let Ok(url) = args.get::<String>("url") {
            builder.http_source(url);
        } else if let Ok(file) = args.get::<String>("file") {
            builder.file_source(file);
        } else if let Ok(serial) = args.get::<String>("serial") {
            builder.force_source_type(SourceType::NativeSdk);
            builder.device_serial(serial);
        } else if let Ok(sdk) = args.get::<String>("sdk")
            && (sdk == "true" || sdk == "1")
        {
            builder.force_source_type(SourceType::NativeSdk);
        }

        let mut center_frequency = 100e6;
        let mut sample_rate = 1e6;
        let mut reference_level = -20.0;

        if let Ok(freq) = args.get::<f64>("freq") {
            center_frequency = freq;
        }
        if let Ok(rate) = args.get::<f64>("rate") {
            sample_rate = rate;
        }
        if let Ok(ref_level) = args.get::<f64>("ref_level") {
            reference_level = ref_level;
        }

        builder.center_frequency(center_frequency);
        builder.span_frequency(sample_rate);
        builder.reference_level(reference_level);

        let source = runtime
            .block_on(builder.build())
            .map_err(|e| seify::Error::Io(std::io::Error::other(e.to_string())))?;

        Ok(Self {
            source: Arc::new(Mutex::new(source)),
            runtime,
            tuning: Arc::new(Mutex::new(Tuning {
                center_frequency,
                sample_rate,
                reference_level,
            })),
        })
    }
}

impl DeviceInfo for AaroniaSeifyDevice {
    fn driver(&self) -> seify::Driver {
        seify::Driver::AaroniaHttp
    }

    fn id(&self) -> std::result::Result<String, seify::Error> {
        Ok("Aaronia Spectran V6".to_string())
    }

    fn info(&self) -> std::result::Result<Args, seify::Error> {
        let mut args = Args::new();
        args.set("driver", "aaronia");
        args.set("label", "Aaronia Spectran V6 (sdr-aaronia-rs)");
        Ok(args)
    }

    fn num_channels(&self, direction: Direction) -> std::result::Result<usize, seify::Error> {
        match direction {
            Direction::Rx => Ok(1),
            Direction::Tx => Ok(0),
        }
    }

    fn full_duplex(&self) -> std::result::Result<bool, seify::Error> {
        Ok(false)
    }
}

impl FrequencyControl for AaroniaSeifyDevice {
    fn frequency(
        &self,
        direction: Direction,
        channel: usize,
    ) -> std::result::Result<f64, seify::Error> {
        if direction != Direction::Rx || channel != 0 {
            return Err(seify::Error::invalid_channel(direction, channel, 1));
        }
        Ok(self.tuning.lock().unwrap().center_frequency)
    }

    fn frequency_range(
        &self,
        direction: Direction,
        channel: usize,
    ) -> std::result::Result<Range, seify::Error> {
        if direction != Direction::Rx || channel != 0 {
            return Err(seify::Error::invalid_channel(direction, channel, 1));
        }
        // Spectran V6 tuning range.
        Ok(Range::new(vec![RangeItem::Interval(10.0, 6.0e9)]))
    }

    fn frequency_components(
        &self,
        _direction: Direction,
        _channel: usize,
    ) -> std::result::Result<Vec<String>, seify::Error> {
        Ok(vec![])
    }

    fn component_frequency_range(
        &self,
        _direction: Direction,
        _channel: usize,
        _name: &str,
    ) -> std::result::Result<Range, seify::Error> {
        Err(seify::Error::unsupported(seify::Capability::Frequency))
    }

    fn component_frequency(
        &self,
        _direction: Direction,
        _channel: usize,
        _name: &str,
    ) -> std::result::Result<f64, seify::Error> {
        Err(seify::Error::unsupported(seify::Capability::Frequency))
    }

    fn set_component_frequency(
        &self,
        _direction: Direction,
        _channel: usize,
        _name: &str,
        _frequency: f64,
    ) -> std::result::Result<(), seify::Error> {
        Err(seify::Error::unsupported(seify::Capability::Frequency))
    }

    fn set_frequency(
        &self,
        direction: Direction,
        channel: usize,
        frequency: f64,
        _args: Args,
    ) -> std::result::Result<(), seify::Error> {
        if direction != Direction::Rx || channel != 0 {
            return Err(seify::Error::invalid_channel(direction, channel, 1));
        }
        let mut source = self.source.lock().unwrap();
        self.runtime
            .block_on(source.set_center_frequency(frequency))
            .map_err(|e| seify::Error::Io(std::io::Error::other(e.to_string())))?;
        self.tuning.lock().unwrap().center_frequency = frequency;
        Ok(())
    }
}

impl SampleRateControl for AaroniaSeifyDevice {
    fn sample_rate(
        &self,
        direction: Direction,
        channel: usize,
    ) -> std::result::Result<f64, seify::Error> {
        if direction != Direction::Rx || channel != 0 {
            return Err(seify::Error::invalid_channel(direction, channel, 1));
        }
        Ok(self.tuning.lock().unwrap().sample_rate)
    }

    fn get_sample_rate_range(
        &self,
        direction: Direction,
        channel: usize,
    ) -> std::result::Result<Range, seify::Error> {
        if direction != Direction::Rx || channel != 0 {
            return Err(seify::Error::invalid_channel(direction, channel, 1));
        }
        // Capped at the IQ-mode constraint (span * 1.5 <= 92.16 MHz).
        Ok(Range::new(vec![RangeItem::Interval(10e3, 61.44e6)]))
    }

    fn set_sample_rate(
        &self,
        direction: Direction,
        channel: usize,
        rate: f64,
    ) -> std::result::Result<(), seify::Error> {
        if direction != Direction::Rx || channel != 0 {
            return Err(seify::Error::invalid_channel(direction, channel, 1));
        }
        let mut source = self.source.lock().unwrap();
        self.runtime
            .block_on(source.set_span_frequency(rate))
            .map_err(|e| seify::Error::Io(std::io::Error::other(e.to_string())))?;
        self.tuning.lock().unwrap().sample_rate = rate;
        Ok(())
    }
}

impl GainControl for AaroniaSeifyDevice {
    fn gain(
        &self,
        direction: Direction,
        channel: usize,
    ) -> std::result::Result<Option<f64>, seify::Error> {
        if direction != Direction::Rx || channel != 0 {
            return Err(seify::Error::invalid_channel(direction, channel, 1));
        }
        Ok(Some(self.tuning.lock().unwrap().reference_level))
    }

    fn gain_elements(
        &self,
        _direction: Direction,
        _channel: usize,
    ) -> std::result::Result<Vec<String>, seify::Error> {
        Ok(vec![])
    }

    fn gain_range(
        &self,
        direction: Direction,
        channel: usize,
    ) -> std::result::Result<Range, seify::Error> {
        if direction != Direction::Rx || channel != 0 {
            return Err(seify::Error::invalid_channel(direction, channel, 1));
        }
        // Reference level in dBm (matches the Soapy plugin's REF gain).
        Ok(Range::new(vec![RangeItem::Interval(-100.0, 10.0)]))
    }

    fn set_gain_element(
        &self,
        _direction: Direction,
        _channel: usize,
        _name: &str,
        _gain: f64,
    ) -> std::result::Result<(), seify::Error> {
        Err(seify::Error::unsupported(seify::Capability::Gain))
    }

    fn gain_element(
        &self,
        _direction: Direction,
        _channel: usize,
        _name: &str,
    ) -> std::result::Result<Option<f64>, seify::Error> {
        Err(seify::Error::unsupported(seify::Capability::Gain))
    }

    fn gain_element_range(
        &self,
        _direction: Direction,
        _channel: usize,
        _name: &str,
    ) -> std::result::Result<Range, seify::Error> {
        Err(seify::Error::unsupported(seify::Capability::Gain))
    }

    fn set_gain(
        &self,
        direction: Direction,
        channel: usize,
        gain: f64,
    ) -> std::result::Result<(), seify::Error> {
        if direction != Direction::Rx || channel != 0 {
            return Err(seify::Error::invalid_channel(direction, channel, 1));
        }
        let mut source = self.source.lock().unwrap();
        self.runtime
            .block_on(source.set_reference_level(gain))
            .map_err(|e| seify::Error::Io(std::io::Error::other(e.to_string())))?;
        self.tuning.lock().unwrap().reference_level = gain;
        Ok(())
    }
}

impl RxDevice for AaroniaSeifyDevice {
    type RxStreamer = AaroniaSeifyRxStreamer;

    fn rx_streamer(
        &self,
        channels: &[usize],
        _args: Args,
    ) -> std::result::Result<Self::RxStreamer, seify::Error> {
        if channels != [0] {
            // `channels.first()` guards the empty-list case, which used
            // to panic inside the error path itself.
            return Err(seify::Error::invalid_channel(
                Direction::Rx,
                channels.first().copied().unwrap_or(0),
                1,
            ));
        }
        Ok(AaroniaSeifyRxStreamer {
            source: self.source.clone(),
            runtime: self.runtime.clone(),
            deferred_overrun: false,
        })
    }
}

impl DynDeviceBackend for AaroniaSeifyDevice {
    fn rx_device(&self) -> Option<&dyn seify::dev::DynRxDevice> {
        Some(self)
    }
}

pub struct AaroniaSeifyRxStreamer {
    source: Arc<Mutex<AaroniaSource>>,
    runtime: Arc<Runtime>,
    deferred_overrun: bool,
}

impl RxStreamer for AaroniaSeifyRxStreamer {
    fn mtu(&self) -> std::result::Result<usize, seify::Error> {
        Ok(65536)
    }

    fn activate_at(&mut self, _time_ns: Option<i64>) -> std::result::Result<(), seify::Error> {
        let mut source = self.source.lock().unwrap();
        self.runtime
            .block_on(source.start_streaming())
            .map_err(|e| seify::Error::Io(std::io::Error::other(e.to_string())))?;
        Ok(())
    }

    fn deactivate_at(&mut self, _time_ns: Option<i64>) -> std::result::Result<(), seify::Error> {
        let mut source = self.source.lock().unwrap();
        self.runtime
            .block_on(source.stop_streaming())
            .map_err(|e| seify::Error::Io(std::io::Error::other(e.to_string())))?;
        Ok(())
    }

    fn read(
        &mut self,
        buffers: &mut [&mut [Complex32]],
        _timeout_us: i64,
    ) -> std::result::Result<usize, seify::Error> {
        if self.deferred_overrun {
            self.deferred_overrun = false;
            return Err(seify::Error::Overrun);
        }

        if buffers.is_empty() {
            return Ok(0);
        }
        let mut source = self.source.lock().unwrap();
        let buf = &mut buffers[0];

        // Honour the caller's timeout (a non-positive value gets a
        // sane default); partial reads within the deadline are
        // returned rather than discarded. The source mutex is held for
        // at most this bounded duration.
        let timeout = if _timeout_us > 0 {
            std::time::Duration::from_micros(_timeout_us as u64)
        } else {
            std::time::Duration::from_millis(100)
        };
        let mut temp = Vec::with_capacity(buf.len());
        let read = self
            .runtime
            .block_on(source.read_samples_deadline(&mut temp, buf.len(), timeout))
            .map_err(|e| {
                if let crate::Error::Io(ref io_err) = e
                    && io_err.kind() == std::io::ErrorKind::TimedOut
                {
                    return seify::Error::Timeout;
                }
                seify::Error::Io(std::io::Error::other(e.to_string()))
            })?;

        buf[..read].copy_from_slice(&temp[..read]);

        if source.take_overrun() {
            if read > 0 {
                self.deferred_overrun = true;
            } else {
                return Err(seify::Error::Overrun);
            }
        }

        Ok(read)
    }
}
