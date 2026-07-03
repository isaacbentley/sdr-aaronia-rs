//! Connect to an Aaronia RTSA HTTP server and stream IQ data.
//! Run with: `cargo run --example http_iq_quickstart --features http`

use sdr_aaronia_rs::{AaroniaConfig, AaroniaSource};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = AaroniaConfig::from_http("http://localhost:54664")
        .center_frequency(2.4e9) // 2.4 GHz
        .reference_level(-20.0);

    println!("Connecting to HTTP stream at http://localhost:54664...");
    let mut source = AaroniaSource::new(config).await?;
    println!("Stream started successfully.");

    let start_time = std::time::Instant::now();
    let mut total_samples = 0;
    let mut buffer = Vec::with_capacity(8192);

    while start_time.elapsed() < Duration::from_secs(5) {
        let read = source.read_samples(&mut buffer, 8192).await?;
        if read == 0 {
            break;
        }

        total_samples += read;
        if total_samples % (8192 * 10) < read && !buffer.is_empty() {
            println!(
                "Received {} samples. Example: i={}, q={}",
                total_samples, buffer[0].re, buffer[0].im
            );
        }
        buffer.clear();
    }

    println!("Streamed a total of {} samples.", total_samples);
    Ok(())
}
