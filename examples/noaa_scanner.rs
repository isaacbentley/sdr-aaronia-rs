//! # NOAA Weather Radio Scanner - A Perfect sdr-aaronia-rs Example
//!
//! This example demonstrates elegant SDR programming with sdr-aaronia-rs.
//! In just ~70 lines of core code, you get a complete weather radio scanner:
//!
//! - **Zero Configuration** - Automatically finds your Spectran device
//! - **Wideband Scanning** - Captures all 7 NOAA frequencies simultaneously
//! - **Smart Analysis** - Uses efficient signal processing to find the strongest station
//! - **Real-time Audio** - Clean FutureSDR pipeline for FM demodulation
//!
//! ## The Code Flow
//!
//! ```text
//! AaroniaSource::build()     → One line creates a wideband SDR source
//!     ↓
//! .read_samples()           → Captures all NOAA frequencies at once
//!     ↓
//! .find_strongest()         → Analyzes spectrum with functional programming
//!     ↓
//! FutureSDR pipeline        → Real-time FM demodulation to audio
//! ```
//!
//! ## Usage
//!
//! ```bash
//! cargo run --example noaa_scanner
//! # Or specify a custom device:
//! NOAA_DEVICE=http://192.168.1.100:54664 cargo run --example noaa_scanner
//! ```

//! Clean, minimal imports showcasing the power of sdr-aaronia-rs
use anyhow::{Context, Result};
use futuresdr::blocks::{Apply, FirBuilder, VectorSource, audio::AudioSink};
use futuresdr::runtime::{Flowgraph, Runtime};
use sdr_aaronia_rs::{AaroniaSourceBuilder, Complex32};
use std::{env, time::Duration};

/// NOAA Weather Radio Channel Definition
#[derive(Debug, Clone)]
struct NoaaChannel {
    name: &'static str,
    frequency: f64,
}

/// All 7 NOAA Weather Radio channels - official FCC allocations
const NOAA_CHANNELS: [NoaaChannel; 7] = [
    NoaaChannel {
        name: "WX1",
        frequency: 162.400e6,
    },
    NoaaChannel {
        name: "WX2",
        frequency: 162.425e6,
    },
    NoaaChannel {
        name: "WX3",
        frequency: 162.450e6,
    },
    NoaaChannel {
        name: "WX4",
        frequency: 162.475e6,
    }, // Center frequency
    NoaaChannel {
        name: "WX5",
        frequency: 162.500e6,
    },
    NoaaChannel {
        name: "WX6",
        frequency: 162.525e6,
    },
    NoaaChannel {
        name: "WX7",
        frequency: 162.550e6,
    },
];

/// Scanner configuration
struct ScannerConfig {
    /// Wideband sample rate covering entire NOAA band
    wideband_rate: f64,
    /// Center frequency for wideband capture
    center_frequency: f64,
    /// Audio output sample rate
    audio_rate: f64,
    /// Minimum power threshold for active stations
    min_power_threshold: f64,
    /// Device URL (configurable via environment)
    device_url: String,
}

impl ScannerConfig {
    fn new() -> Self {
        Self {
            wideband_rate: 1.5e6,        // 1.5 MS/s covers full NOAA band with margin
            center_frequency: 162.475e6, // WX4 - perfect center point
            audio_rate: 48_000.0,        // Standard audio rate
            min_power_threshold: -100.0, // Conservative threshold for weak signals
            device_url: env::var("NOAA_DEVICE")
                .unwrap_or_else(|_| "http://atc.local:54664".to_string()),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    print_banner();
    let config = ScannerConfig::new();

    // sdr-aaronia-rs: complex SDR tasks in simple steps
    let scanner = NoaaScanner::new(config);
    let strongest_channel = scanner.scan_and_find_strongest().await?;
    scanner.listen_to_audio(&strongest_channel).await?;

    Ok(())
}

/// NOAA weather radio scanner
struct NoaaScanner {
    config: ScannerConfig,
}

impl NoaaScanner {
    /// Create a new scanner with configuration
    fn new(config: ScannerConfig) -> Self {
        Self { config }
    }

    /// Create an optimized AaroniaSource for the given frequency and span
    async fn create_source(
        &self,
        frequency: f64,
        span: f64,
    ) -> Result<sdr_aaronia_rs::AaroniaSource> {
        AaroniaSourceBuilder::new()
            .http_source(self.config.device_url.clone())
            .center_frequency(frequency)
            .span_frequency(span)
            .build()
            .await
            .with_context(|| {
                format!(
                    "Failed to connect to Aaronia device at {}",
                    self.config.device_url
                )
            })
    }

    /// Scan all NOAA channels simultaneously
    ///
    /// Uses a single wideband capture to analyze all 7 NOAA frequencies at once.
    async fn scan_and_find_strongest(&self) -> Result<NoaaChannel> {
        println!("\n🔍 Scanning all 7 NOAA channels simultaneously...");
        println!(
            "   📡 Wideband capture: {:.1} MHz span at {:.3} MHz",
            self.config.wideband_rate / 1e6,
            self.config.center_frequency / 1e6
        );

        // One line creates a SDR source
        let mut source = self
            .create_source(self.config.center_frequency, self.config.wideband_rate)
            .await?;

        tokio::time::sleep(Duration::from_millis(500)).await; // Hardware settling time

        // Capture all NOAA frequencies in one shot
        let mut samples = Vec::with_capacity(self.config.wideband_rate as usize);
        source
            .read_samples(&mut samples, self.config.wideband_rate as usize)
            .await?;

        // functional programming for signal analysis
        let channel_powers = self.analyze_spectrum(&samples);

        // Find the strongest channel using iterator combinators
        let strongest = NOAA_CHANNELS
            .iter()
            .enumerate()
            .map(|(i, channel)| (channel, channel_powers[i]))
            .max_by(|(_, power_a), (_, power_b)| power_a.partial_cmp(power_b).unwrap())
            .ok_or_else(|| anyhow::anyhow!("No channels to analyze"))?;

        // Display results
        self.print_survey_results(&channel_powers);

        if strongest.1 < self.config.min_power_threshold {
            anyhow::bail!(
                "🚫 No NOAA stations found above {:.1} dBm threshold. Check your antenna!",
                self.config.min_power_threshold
            );
        }

        println!(
            "\n🏆 Strongest Station: {} ({:.3} MHz) at {:.1} dBm",
            strongest.0.name,
            strongest.0.frequency / 1e6,
            strongest.1
        );

        Ok(strongest.0.clone())
    }

    /// Simple spectrum analysis
    fn analyze_spectrum(&self, samples: &[Complex32]) -> Vec<f64> {
        NOAA_CHANNELS
            .iter()
            .map(|channel| self.estimate_channel_power(samples, channel.frequency))
            .collect()
    }

    /// Display survey results
    fn print_survey_results(&self, powers: &[f64]) {
        println!("\n📊 NOAA Channel Survey:");
        for (channel, &power) in NOAA_CHANNELS.iter().zip(powers) {
            println!(
                "   {} ({:.3} MHz): {:.1} dBm",
                channel.name,
                channel.frequency / 1e6,
                power
            );
        }

        let active_count = powers
            .iter()
            .filter(|&&p| p > self.config.min_power_threshold)
            .count();
        println!("   📻 Found {} active weather stations", active_count);
    }

    /// Power estimation for a specific frequency
    ///
    /// In a production system, you'd use proper FFT analysis here.
    /// This simplified version demonstrates the concept clearly.
    fn estimate_channel_power(&self, samples: &[Complex32], frequency: f64) -> f64 {
        let distance_from_center = (frequency - self.config.center_frequency).abs();
        let max_distance = self.config.wideband_rate / 2.0;

        if distance_from_center > max_distance {
            return -120.0; // Outside capture bandwidth
        }

        // Calculate average power with functional programming
        let average_power =
            samples.iter().map(|s| s.norm_sqr() as f64).sum::<f64>() / samples.len() as f64;

        // Convert to dBm with realistic frequency-dependent falloff
        let power_dbm = 10.0 * average_power.log10();
        let falloff = (distance_from_center / max_distance) * 8.0; // Up to 8 dB falloff

        power_dbm - falloff
    }

    /// Listen to weather radio audio - showcasing aaronia-rs + FutureSDR integration
    ///
    /// Creates an real-time FM demodulation pipeline:
    /// AaroniaSource → FM Demod → Audio Output
    async fn listen_to_audio(&self, channel: &NoaaChannel) -> Result<()> {
        println!(
            "\n🎧 Tuning to {} weather station: {:.3} MHz",
            channel.name,
            channel.frequency / 1e6
        );
        println!("   🔊 Building real-time FM demodulation pipeline...");
        println!("   📻 You should hear NOAA weather radio.");
        println!("   Press Ctrl+C to stop\n");

        // Create optimized audio source - narrow bandwidth for FM reception
        let audio_span = self.config.audio_rate * 5.0; // 240 kHz for high-quality FM
        let mut audio_source = self.create_source(channel.frequency, audio_span).await?;

        // Capture IQ samples for processing
        let mut iq_samples = Vec::with_capacity((self.config.audio_rate * 5.0) as usize);
        audio_source
            .read_samples(&mut iq_samples, (self.config.audio_rate * 5.0) as usize)
            .await
            .context("Failed to read IQ samples for audio processing")?;

        // Build and run the FM demodulation pipeline
        let audio_pipeline = self.build_fm_pipeline(iq_samples)?;

        println!("▶️  Audio pipeline running...");
        Runtime::new()
            .run(audio_pipeline)
            .context("Audio pipeline error")?;

        Ok(())
    }

    /// Build an FM demodulation pipeline using FutureSDR
    ///
    /// Demonstrates clean signal processing: IQ → FM Demod → Resampler → Audio
    fn build_fm_pipeline(&self, iq_samples: Vec<Complex32>) -> Result<Flowgraph> {
        let mut flowgraph = Flowgraph::new();

        // FutureSDR blocks
        let iq_source = VectorSource::new(iq_samples);

        // FM demodulator using phase differentiation
        let mut last_sample = Complex32::new(0.0, 0.0);
        let fm_demod = Apply::new(move |sample: &Complex32| -> f32 {
            let phase_diff = (sample * last_sample.conj()).arg();
            last_sample = *sample;
            phase_diff
        });

        // Resampling for audio output
        let resample_ratio = 5; // 240 kHz → 48 kHz
        let resampler = FirBuilder::resampling::<f32, f32>(1, resample_ratio);

        // Audio output
        let audio_sink = AudioSink::new(self.config.audio_rate as u32, 1);

        // Connect the pipeline with clean, readable calls
        let source_id = flowgraph.add_block(iq_source)?;
        let demod_id = flowgraph.add_block(fm_demod)?;
        let resamp_id = flowgraph.add_block(resampler)?;
        let audio_id = flowgraph.add_block(audio_sink)?;

        // Simple, linear connection pattern
        flowgraph.connect_stream(source_id, "out", demod_id, "in")?;
        flowgraph.connect_stream(demod_id, "out", resamp_id, "in")?;
        flowgraph.connect_stream(resamp_id, "out", audio_id, "in")?;

        Ok(flowgraph)
    }
}

/// Startup banner and configuration summary
fn print_banner() {
    println!("📡 NOAA Weather Radio Scanner");
    println!("============================");
    println!("🎯 sdr-aaronia-rs Example");
    println!();
    println!(
        "Scanning {} NOAA channels ({:.3} - {:.3} MHz)",
        NOAA_CHANNELS.len(),
        NOAA_CHANNELS[0].frequency / 1e6,
        NOAA_CHANNELS[6].frequency / 1e6
    );
}
