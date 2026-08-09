//! Channel hopping using the native `sdr-source` traits.
//!
//! This example shows how to configure `AaroniaSdrSource` with a dwell advice
//! controller and multiple frequency channels, automatically hopping frequencies
//! while receiving unified IQ data.
//!
//! Note: Requires the `sdr-source` feature and a connected device.

#[cfg(feature = "sdr-source")]
fn main() -> anyhow::Result<()> {
    use sdr_aaronia_rs::sdr_source::{DwellAdvice, SdrSource, SourceConfig};
    use sdr_aaronia_rs::sdr_source_impl::{AaroniaBackend, AaroniaSdrSource};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    struct DummyAdvice;
    impl DwellAdvice for DummyAdvice {
        fn latest_signal_at(&self, _freq_key: u64) -> Option<Instant> {
            None
        }
    }

    println!("Starting Aaronia Channel Hopping Example");

    // Configure the source hopping plan
    let source_config = SourceConfig {
        channels_hz: vec![446.0e6, 446.1e6, 446.2e6],
        sample_rate_hz: 10.0e6,
        dwell_min: Duration::from_millis(200),
        dwell_max: Duration::from_millis(200),
        dwell_extension: Duration::ZERO,
    };

    println!("Initializing Aaronia SDR Source with 3 channels...");
    // AaroniaSdrSource implements `SdrSource` providing a synchronized, hopping stream
    let source = Box::new(AaroniaSdrSource {
        backend: AaroniaBackend::Http("http://localhost:54664".to_string()),
        center_frequency_hz: 446.0e6,
        reference_level_dbm: -20.0,
        block_size: 8192,
        // None = library default (binary Float32). Set Some(StreamFormat::
        // Int16) to halve network bandwidth at high sample rates.
        stream_format: None,
    });

    let advice = Arc::new(DummyAdvice);
    let handle = source.start(source_config, advice)?;

    println!("Streaming IQ data and hopping channels...");
    let start_time = Instant::now();
    let mut total_samples = 0;
    let mut hops = 0;

    // Loop for 5 seconds
    while start_time.elapsed() < Duration::from_secs(5) {
        if let Ok(packet) = handle.receiver.recv_timeout(Duration::from_millis(100)) {
            total_samples += packet.samples.len();
            println!(
                "Received packet with {} samples at frequency {:.1} MHz",
                packet.samples.len(),
                packet.center_frequency_hz / 1e6
            );
            hops += 1;
        }
    }

    println!("Stopping stream...");
    (handle.stop)();
    (handle.wait)();

    println!(
        "Streamed {} samples over {} received packets.",
        total_samples, hops
    );
    Ok(())
}

#[cfg(not(feature = "sdr-source"))]
fn main() {
    println!("This example requires the 'sdr-source' feature.");
    println!("Run with: cargo run --example channel_hopping --features sdr-source");
}
