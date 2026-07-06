use sdr_aaronia_rs::{AaroniaConfig, AaroniaSource};
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let freq = args
        .get(1)
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(57_000_000.0);
    let rate = args
        .get(2)
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(10_000_000.0);
    let url = "http://atc.local:54664";

    let config = AaroniaConfig::from_http(url)
        .center_frequency(freq)
        .sample_rate_hz(rate)
        .reference_level(-20.0);

    eprintln!(
        "Connecting to HTTP stream at {} (freq={} rate={})...",
        url, freq, rate
    );
    let mut source = AaroniaSource::new(config).await?;
    let info = source.get_source_info();
    eprintln!("Stream started successfully. Info: {:?}", info);

    let mut stdout = std::io::stdout().lock();
    let mut buffer = Vec::with_capacity(8192);
    let mut total_samples = 0;
    let mut last_print = std::time::Instant::now();

    loop {
        let read = source.read_samples(&mut buffer, 8192).await?;
        if read == 0 {
            break;
        }

        // Calculate power to verify signal
        if last_print.elapsed().as_secs() >= 1 {
            let mut power_sum = 0.0;
            for sample in &buffer {
                power_sum += sample.norm_sqr();
            }
            let avg_power = power_sum / buffer.len() as f32;
            let dbfs = 10.0 * avg_power.max(1e-12).log10();
            eprintln!(
                "[DEBUG] Stream alive: transferred {} total samples. Current avg power: {:.1} dBFS",
                total_samples + read,
                dbfs
            );
            last_print = std::time::Instant::now();
        }

        total_samples += read;

        let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr() as *const u8, read * 8) };
        if stdout.write_all(bytes).is_err() {
            break; // pipe broken
        }
        buffer.clear();
    }

    Ok(())
}
