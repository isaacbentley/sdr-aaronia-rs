//! Basic example of using the Aaronia Native SDK integration for TX.
//!
//! This example demonstrates:
//! - Setting up the transmit path using `SdkSink`.
//! - Using the device master stream time to correctly pace transmission packets.
//! - Generating a swept test signal (chirp).
//! - Utilizing packet boundaries (`STREAM_START`/`STREAM_END`).
//!
//! Note: Requires the native-sdk feature and an installed Aaronia RTSA-Suite PRO.
//! Note: Transmit capability requires an SDR hardware unit with TX enabled (e.g., standard Spectran V6, not ECO).

#[cfg(all(
    feature = "native-sdk",
    any(target_os = "windows", target_os = "linux")
))]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use sdr_aaronia_rs::Complex32;
    use sdr_aaronia_rs::native_sdk::TxBurst;
    use sdr_aaronia_rs::native_sdk::tx_flags;
    use sdr_aaronia_rs::sdk_sink::{SdkSink, SdkSinkConfig};
    use tracing::info;

    // Initialize logging
    tracing_subscriber::fmt::init();

    info!("🚀 Starting Aaronia Native SDK Transmit Example");

    // Configure the sink
    let config = SdkSinkConfig {
        device_type: "spectranv6/iqtransmitter".to_string(),
        center_frequency: 2.44e9, // 2.44 GHz
        span_frequency: 10e6,     // 10 MHz span/sample rate
        trans_gain: -20.0,        // -20 dB
        timeout: std::time::Duration::from_secs(30),
    };

    let mut sdk_sink = SdkSink::with_config(config.clone());

    // Initialize the SDK and discover devices
    sdk_sink.initialize().await?;

    info!("📡 Opening device and starting stream...");

    // This connects, configures, and starts the device in TX mode
    // (If the device does not have TX capability, or is disconnected, this will fail here).
    if let Err(e) = sdk_sink.start_streaming().await {
        println!("❌ Failed to start streaming: {}", e);
        println!("Please ensure a TX-capable Spectran V6 is connected.");
        return Ok(());
    }

    info!("✅ Stream started. Generating test signal...");

    // Generate a LoRa-like up-chirp signal
    const SAMPLES_PER_BURST: usize = 16384;
    const NUM_BURSTS: usize = 100;

    let mut iq_buffer = vec![Complex32::new(0.0, 0.0); SAMPLES_PER_BURST];
    let mut phase: f64 = 0.0;
    let mut symbol_sample_idx = 0;

    // LoRa-like chirp parameters
    let bandwidth: f64 = 500e3; // 500 kHz bandwidth
    let sf: u32 = 10; // Spreading factor 10
    let symbol_length = 1usize << sf; // 1024 samples per symbol

    // Read the current master stream time from the device
    // This is required to correctly schedule our packets on the device's FPGA
    let mut current_time = sdk_sink.get_master_stream_time()?;

    // Pre-buffer by scheduling the first packet 200 ms in the future
    current_time += 0.2;

    let duration_per_burst = SAMPLES_PER_BURST as f64 / config.span_frequency;

    for i in 0..NUM_BURSTS {
        for j in 0..SAMPLES_PER_BURST {
            // Normalized time within the symbol [0.0, 1.0)
            let t_sym = (symbol_sample_idx as f64) / (symbol_length as f64);

            // Instantaneous frequency for a base up-chirp goes from -BW/2 to +BW/2
            let instantaneous_freq = -bandwidth / 2.0 + bandwidth * t_sym;

            // Phase increment per sample: 2 * pi * f * dt
            // where dt = 1 / sample_rate
            let phase_inc = std::f64::consts::TAU * instantaneous_freq / config.span_frequency;

            phase = (phase + phase_inc) % std::f64::consts::TAU;

            // Full scale is roughly 1.0 depending on transgain
            iq_buffer[j] = Complex32::new(phase.cos() as f32, phase.sin() as f32);

            symbol_sample_idx = (symbol_sample_idx + 1) % symbol_length;
        }

        // Determine packet flags
        let mut flags = 0;
        if i == 0 {
            flags |= tx_flags::STREAM_START | tx_flags::SEGMENT_START;
        }
        if i == NUM_BURSTS - 1 {
            flags |= tx_flags::STREAM_END | tx_flags::SEGMENT_END;
        }

        let burst = TxBurst {
            start_time: current_time,
            end_time: current_time + duration_per_burst,
            center_frequency_hz: config.center_frequency,
            sample_rate_hz: config.span_frequency,
            flags,
        };

        // Send the packet
        sdk_sink.write_samples(0, burst, &iq_buffer)?;

        current_time += duration_per_burst;

        // Pace our generation loop slightly so we don't overflow the device's packet queue
        // A real application would calculate queue depth by comparing master stream time
        // with the `current_time` variable.
        std::thread::sleep(std::time::Duration::from_millis(
            (duration_per_burst * 1000.0 * 0.8) as u64,
        ));

        if i % 10 == 0 {
            info!("Sent burst {}/{}", i + 1, NUM_BURSTS);
        }
    }

    info!("🛑 Stopping streaming");
    sdk_sink.stop_streaming().await?;

    info!("✅ Example completed successfully");
    Ok(())
}

#[cfg(not(all(
    feature = "native-sdk",
    any(target_os = "windows", target_os = "linux")
)))]
fn main() {
    println!("This example requires the 'native-sdk' feature and Windows or Linux.");
    println!("Run with: cargo run --example native_sdk_transmit --features native-sdk");
}
