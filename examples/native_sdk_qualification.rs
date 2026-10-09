//! Receive-only ECO qualification: explicit serial, span, carry and stop/reopen.
//! Run only with the radio released by other apps; no TX is opened.
#[cfg(all(
    feature = "native-sdk",
    any(target_os = "windows", target_os = "linux")
))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    use sdr_aaronia_rs::{Error, SourceType, SpectranSourceBuilder};
    use std::time::{Duration, Instant};
    let serial = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("Pass the explicit USB serial"))?;
    let seconds: u64 = std::env::args()
        .nth(2)
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(8);
    anyhow::ensure!(
        (2..=30).contains(&seconds),
        "cycle duration must be 2–30 seconds"
    );
    let mut builder = SpectranSourceBuilder::new();
    builder
        .force_source_type(SourceType::NativeSdk)
        .device_serial(serial)
        .center_frequency_hz(280e6)
        .reference_level_dbm(-50.0)
        .rf_span_hz(44e6);
    let until = Instant::now() + Duration::from_secs(15);
    let mut source = loop {
        match builder.build().await {
            Ok(source) => break source,
            Err(Error::DeviceNotReady { .. } | Error::DeviceNotFound { .. })
                if Instant::now() < until =>
            {
                tokio::time::sleep(Duration::from_millis(500)).await
            }
            Err(error) => return Err(error.into()),
        }
    };
    println!("{}", source.device_capabilities().await.to_json()?);
    let mut buffer = Vec::new();
    for (index, span) in [44e6, 20e6, 10e6, 5e6, 44e6].into_iter().enumerate() {
        if index > 0 {
            source.set_rf_span_hz(span).await?;
        }
        source.start_streaming().await?;
        source.start_streaming().await?; // Must be idempotent.
        let until = Instant::now() + Duration::from_secs(seconds);
        let mut samples = 0usize;
        let mut reads = 0;
        while Instant::now() < until {
            match source.read_samples(&mut buffer, 65536).await {
                Ok(n) => {
                    samples += n;
                    reads += 1;
                }
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(error) => return Err(error.into()),
            }
        }
        let info = source.source_info();
        println!(
            "{{\"cycle\":{index},\"requested_span_hz\":{span},\"sample_rate_hz\":{},\"rf_span_hz\":{},\"center_hz\":{},\"samples\":{samples},\"reads\":{reads},\"drops\":{}}}",
            info.sample_rate_hz,
            info.bandwidth_hz,
            info.center_frequency_hz,
            source.cumulative_drops()
        );
        anyhow::ensure!(samples > 0, "no IQ in cycle {index}");
        anyhow::ensure!(
            (info.bandwidth_hz - span).abs() < 1.0,
            "RF width differs from request"
        );
        source.stop_streaming().await?;
        source.stop_streaming().await?; // Must not close twice.
        // Configuration while closed is deferred; start reopens the same serial.
        source
            .set_center_frequency_hz(280e6 + (index + 1) as f64 * 1e6)
            .await?;
        source.set_reference_level_dbm(-50.0 + index as f64).await?;
    }
    source.stop_streaming().await?;
    println!("{{\"qualification_complete\":true}}");
    Ok(())
}
#[cfg(not(all(
    feature = "native-sdk",
    any(target_os = "windows", target_os = "linux")
)))]
fn main() {
    eprintln!("Requires native-sdk on Windows or Linux");
}
