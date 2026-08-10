//! Basic example of using the Aaronia Native SDK integration.
//!
//! This example demonstrates:
//! - SDK initialization (`Init_With_Path` `Open` `RescanDevices`)
//! - Device **family** enumeration vs. mode-qualified open
//!   (per the official RTSA-API-Samples: `EnumDevice("spectranv6")` then
//!   `OpenDevice("spectranv6/raw", serial)`)
//! - IQ-mode configuration with the receiver-clock constraint check
//! - The polling loop `read_samples` performs internally (5 ms sleeps,
//!   500 ms deadline), matching IQReceiverEco.cpp / RawIQ.cpp
//! - Optional non-blocking liveness checks via `avail_packets` and the
//!   typed `set_decimation_factor` helper.
//!
//! Note: Requires the native-sdk feature and an installed Aaronia RTSA-Suite PRO

#[cfg(all(
    feature = "native-sdk",
    any(target_os = "windows", target_os = "linux")
))]
fn main() -> anyhow::Result<()> {
    use sdr_aaronia_rs::native_sdk::NativeSdkSource;
    use tracing::info;

    // Initialize logging. The library emits `tracing` events (including
    // the native-SDK path), so use a tracing subscriber here.
    tracing_subscriber::fmt::init();

    unsafe {
        info!("Starting Aaronia Native SDK Example");

        // Create and initialize the SDK source
        let mut sdk_source = NativeSdkSource::new()?;
        sdk_source.initialize()?;

        // `AARTSAAPI_EnumDevice` takes the bare device family — *not* a
        // mode-qualified string. Passing "spectranv6/raw" silently returns
        // zero devices on the official SDK. See EnumDevices.cpp:33.
        let device_family = "spectranv6";
        let open_mode = "spectranv6/raw";

        let devices = sdk_source.find_devices(device_family)?;

        if devices.is_empty() {
            println!("No Spectran V6 devices found. Please connect a device and try again.");
            return Ok(());
        }

        // Use the first available device. `serial_number` is a fixed-size
        // wide-char array (`[WideChar; 120]`, where `WideChar` is `u16` on
        // Windows and `u32` on Linux) supplied by the SDK directly — already
        // the slice type `open_device` wants, so we pass it through with no
        // conversion or allocation.
        let device_info = &devices[0];

        info!(
            "Opening device: {}",
            NativeSdkSource::get_device_serial(device_info)
        );
        sdk_source.open_device(open_mode, &device_info.serial_number)?;

        // Configure for UHF amateur band (446 MHz, 10 MHz span, -30 dBm).
        // `configure_iq_receiver` validates the IQ Mode constraint
        // (`span * 1.5 <= clock`) before returning.
        let (center_freq, span_freq, ref_level) = (446.0e6, 10.0e6, -30.0);
        info!(
            "Configuring IQ receiver: {} Hz center, {} Hz span, {} dBm ref level",
            center_freq, span_freq, ref_level
        );
        // `None` keeps the Rx1 default; pass Some(RxChannel::Rx2) or
        // Some(RxChannel::Rx1And2) on a full V6 to select inputs.
        sdk_source.configure_iq_receiver(center_freq, span_freq, ref_level, None)?;

        // Optional: `set_decimation_factor` accepts powers of two in
        // [1, 512]; factor 1 (`Full`) keeps the native rate. The helper
        // only operates on `spectranv6/raw`, so eco devices report it as
        // not applicable.
        if let Err(e) = sdk_source.set_decimation_factor(1) {
            info!("Decimation not applicable on this device: {}", e);
        }

        // Start streaming
        info!("Starting IQ sample streaming");
        sdk_source.start_streaming()?;

        // Collect some samples. `read_samples` itself polls `GetPacket`
        // every 5 ms up to a 500 ms deadline, matching the canonical
        // sample loop, so the outer 100 ms sleep is just for pacing the
        // logs.
        info!("Collecting sample data...");

        // Pre-allocate a reusable buffer. `read_samples` appends into
        // this vector and returns the number of samples written, so we
        // clear it each iteration instead of allocating a new Vec.
        let mut buffer = Vec::with_capacity(1024);

        for i in 0..10 {
            buffer.clear();
            let n = sdk_source.read_samples(&mut buffer, 1024)?;
            if n > 0 {
                println!("Batch {}: Got {} IQ samples", i + 1, n);

                // Calculate some basic statistics. The f32 components are
                // in volts but the front-end gain depends on the open
                // mode (see `NativeSdkSource::read_samples` doc-comment
                // for the iqreceiver vs. raw scaling).
                let avg_power: f32 = buffer.iter().map(|s| s.norm_sqr()).sum::<f32>() / n as f32;

                println!("  Average power: {:.2} dB", 10.0 * avg_power.log10());
            } else {
                println!("Batch {}: No samples available", i + 1);
            }

            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        // Stop streaming
        info!("Stopping streaming");
        sdk_source.stop_streaming()?;

        info!("Example completed successfully");
        Ok(())
    }
}

#[cfg(not(all(
    feature = "native-sdk",
    any(target_os = "windows", target_os = "linux")
)))]
fn main() {
    println!("This example requires the 'native-sdk' feature and Windows or Linux.");
    println!("Run with: cargo run --example native_sdk_basic --features native-sdk");
}
