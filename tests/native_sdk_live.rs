//! Exhaustive live validation of the native-SDK backend against real
//! hardware.
//!
//! Every test here is `#[ignore]`d: they need a Spectran V6 attached to
//! *this* machine and RTSA-Suite PRO installed, and they take exclusive
//! control of the device. The HTTP suite in `live_smoke.rs` covers the
//! network backend; this file covers the path that only exists on
//! Windows and Linux.
//!
//! ```sh
//! cargo test --features native-sdk --test native_sdk_live -- --ignored --nocapture
//! ```
//!
//! Optional environment overrides:
//!
//! - `AARONIA_SDK_SERIAL` — pick a specific device (default: first found)
//! - `AARONIA_SDK_CENTER` — centre frequency in Hz (default 2.44e9)
//! - `AARONIA_SDK_TRIALS` — repetitions per ladder rung (default 5)
//! - `AARONIA_SDK_SOAK_SECS` — long-run duration (default 60)

#![cfg(all(
    feature = "native-sdk",
    any(target_os = "windows", target_os = "linux")
))]

use sdr_aaronia_rs::Complex32;
use sdr_aaronia_rs::unified_source::{AaroniaConfig, AaroniaSource, SourceType};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, MutexGuard};

// ---------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------

/// The device is exclusive — a second open fails while the first holds
/// it. `cargo test` runs test functions on parallel threads by default,
/// so serialise here rather than depending on the caller remembering
/// `--test-threads=1`.
///
/// This is `tokio`'s mutex rather than `std`'s because the guard is held
/// across `.await` points for the whole duration of a capture — that is
/// the point of it. A `std::sync::MutexGuard` held that way is what
/// `clippy::await_holding_lock` exists to catch. Tokio's guard also has
/// no poisoning, so one panicking test cannot cascade into "every later
/// test failed to acquire the device".
fn lock_cell() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

async fn device_lock() -> MutexGuard<'static, ()> {
    lock_cell().lock().await
}

/// The blocking counterpart, for the plain `#[test]` functions that have
/// no runtime to await on.
fn device_lock_blocking() -> MutexGuard<'static, ()> {
    lock_cell().blocking_lock()
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn center_hz() -> f64 {
    env_f64("AARONIA_SDK_CENTER", 2.44e9)
}

/// Base configuration pinned to the native SDK. `force_native_sdk`
/// means a missing SDK is a hard error here rather than a silent
/// fallback to HTTP — which is exactly what these tests must assert.
fn sdk_config(sample_rate_hz: f64) -> AaroniaConfig {
    let mut cfg = AaroniaConfig::default()
        .force_native_sdk()
        .center_frequency_hz(center_hz())
        .sample_rate_hz(sample_rate_hz)
        .reference_level_dbm(-20.0)
        .read_timeout(Duration::from_secs(10));
    cfg.device_serial = std::env::var("AARONIA_SDK_SERIAL").ok();
    cfg
}

/// Open, stream briefly, and report how many samples arrived. Returns
/// the error rather than panicking so callers can characterise
/// failures instead of aborting the run on the first one.
async fn try_capture(sample_rate_hz: f64, samples_wanted: usize) -> Result<(usize, f64), String> {
    let mut source = AaroniaSource::new(sdk_config(sample_rate_hz))
        .await
        .map_err(|e| format!("open: {e}"))?;

    // A source that quietly came up on another backend would make every
    // downstream assertion meaningless.
    assert_eq!(
        source.get_source_info().source_type,
        SourceType::NativeSdk,
        "force_native_sdk must not fall back to another backend"
    );

    source
        .start_streaming()
        .await
        .map_err(|e| format!("start: {e}"))?;

    let mut buf: Vec<Complex32> = Vec::new();
    let mut total = 0usize;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last_err = None;

    while total < samples_wanted && Instant::now() < deadline {
        match source.read_samples(&mut buf, samples_wanted - total).await {
            Ok(0) => continue,
            Ok(n) => total += n,
            Err(e) => {
                last_err = Some(format!("read: {e}"));
                break;
            }
        }
    }

    // Read before stopping: the report is cleared when streaming stops.
    let reported = source.sample_rate_hz();
    let _ = source.stop_streaming().await;

    match (total, last_err) {
        (0, Some(e)) => Err(e),
        (0, None) => Err("read: no samples within 10 s".to_string()),
        (n, _) => Ok((n, reported)),
    }
}

// ---------------------------------------------------------------------
// A. Detection and library
// ---------------------------------------------------------------------

#[test]
#[ignore = "requires RTSA-Suite PRO installed on this machine"]
fn sdk_is_detected_on_this_machine() {
    let path = sdr_aaronia_rs::get_sdk_library_path();
    println!("SDK path    : {:?}", sdr_aaronia_rs::get_sdk_path());
    println!("SDK library : {path:?}");
    assert!(
        sdr_aaronia_rs::is_sdk_installed(),
        "SDK not detected — set AARONIA_SDK_PATH to the install directory"
    );
    assert!(path.is_some(), "library path must resolve once detected");
}

// ---------------------------------------------------------------------
// B. Enumeration and identity
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires an attached Spectran V6"]
async fn opens_device_and_reports_native_backend() {
    let _guard = device_lock().await;

    let source = AaroniaSource::new(sdk_config(15.36e6))
        .await
        .expect("a Spectran V6 must be attached and free");

    let info = source.get_source_info();
    println!(
        "source_type={:?} center={} Hz rate={} S/s serial={:?}",
        info.source_type, info.center_frequency_hz, info.sample_rate_hz, info.device_serial
    );
    assert_eq!(info.source_type, SourceType::NativeSdk);
}

#[tokio::test]
#[ignore = "requires an attached Spectran V6"]
async fn unknown_serial_fails_with_an_actionable_message() {
    let _guard = device_lock().await;

    let mut cfg = sdk_config(15.36e6);
    cfg.device_serial = Some("NO-SUCH-SERIAL-0000".to_string());

    let err = AaroniaSource::new(cfg)
        .await
        .err()
        .expect("a bogus serial must not open a device");
    let msg = err.to_string();
    println!("error: {msg}");

    // The point of the assertion: the message must name the thing that
    // was wrong. "No Spectran V6 devices found" would be a lie here —
    // a device *is* present, it just doesn't carry that serial.
    assert!(
        msg.contains("NO-SUCH-SERIAL-0000"),
        "error should name the serial that was not found, got: {msg}"
    );
}

// ---------------------------------------------------------------------
// C. Lifecycle robustness
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires an attached Spectran V6; ~1 min"]
async fn repeated_open_close_cycles_do_not_degrade() {
    let _guard = device_lock().await;

    const CYCLES: usize = 10;
    let mut failures = Vec::new();

    for cycle in 1..=CYCLES {
        match try_capture(15.36e6, 65_536).await {
            Ok((n, _)) => println!("cycle {cycle:2}: ok, {n} samples"),
            Err(e) => {
                println!("cycle {cycle:2}: FAILED — {e}");
                failures.push((cycle, e));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{}/{CYCLES} open/close cycles failed — the device or the SDK \
         handle is not being released cleanly: {failures:?}",
        failures.len()
    );
}

#[tokio::test]
#[ignore = "requires an attached Spectran V6"]
async fn read_before_start_streaming_errors_rather_than_hanging() {
    let _guard = device_lock().await;

    let mut source = AaroniaSource::new(sdk_config(15.36e6))
        .await
        .expect("device must open");

    let mut buf: Vec<Complex32> = Vec::new();
    let result =
        tokio::time::timeout(Duration::from_secs(15), source.read_samples(&mut buf, 1024)).await;

    match result {
        Err(_) => panic!("read_samples before start_streaming blocked past its timeout"),
        Ok(Ok(n)) => println!("read returned {n} samples without an explicit start"),
        Ok(Err(e)) => println!("read correctly refused: {e}"),
    }
}

#[tokio::test]
#[ignore = "requires an attached Spectran V6"]
async fn stop_without_start_is_harmless() {
    let _guard = device_lock().await;

    let mut source = AaroniaSource::new(sdk_config(15.36e6))
        .await
        .expect("device must open");

    // Must not panic and must not wedge the device: the following open
    // in the next test would fail if it did.
    let result = source.stop_streaming().await;
    println!("stop_streaming() without a start: {result:?}");
}

// ---------------------------------------------------------------------
// D. Configuration
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires an attached Spectran V6; ~1 min"]
async fn center_frequency_sweep() {
    let _guard = device_lock().await;

    // Spread across the V6 range, avoiding the very edges.
    let freqs = [100e6, 433.92e6, 868e6, 1.575e9, 2.44e9, 5.8e9];
    let mut failures = Vec::new();

    for f in freqs {
        let mut cfg = sdk_config(15.36e6);
        cfg.center_frequency_hz = f;

        match AaroniaSource::new(cfg).await {
            Ok(source) => {
                let seen = source.get_source_info().center_frequency_hz;
                println!("{:>10.3} MHz -> reported {:>10.3} MHz", f / 1e6, seen / 1e6);
            }
            Err(e) => {
                println!("{:>10.3} MHz -> FAILED: {e}", f / 1e6);
                failures.push((f, e.to_string()));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "centre frequencies inside the documented range must tune: {failures:?}"
    );
}

#[tokio::test]
#[ignore = "requires an attached Spectran V6"]
async fn mid_stream_retune_keeps_samples_flowing() {
    let _guard = device_lock().await;

    let mut source = AaroniaSource::new(sdk_config(15.36e6))
        .await
        .expect("device must open");
    source.start_streaming().await.expect("start");

    // Poll to a deadline rather than trusting a single read: the first
    // read after `start_streaming` can land before the device has
    // produced its first packet, and one packet is all a read returns.
    let mut buf: Vec<Complex32> = Vec::new();
    let mut before = 0usize;
    let deadline = Instant::now() + Duration::from_secs(10);
    while before == 0 && Instant::now() < deadline {
        before = source.read_samples(&mut buf, 65_536).await.unwrap_or(0);
        buf.clear();
    }
    assert!(
        before > 0,
        "must receive samples within 10 s before retuning"
    );

    source
        .set_center_frequency_hz(1.09e9)
        .await
        .expect("mid-stream retune");

    let mut after = 0usize;
    let deadline = Instant::now() + Duration::from_secs(10);
    while after == 0 && Instant::now() < deadline {
        after = source.read_samples(&mut buf, 65_536).await.unwrap_or(0);
        buf.clear();
    }

    let _ = source.stop_streaming().await;
    println!("before={before} samples, after retune={after} samples");
    assert!(after > 0, "stream must recover after a mid-stream retune");
}

// ---------------------------------------------------------------------
// E. Span ladder characterisation  — the headline test
// ---------------------------------------------------------------------

/// Records, rather than asserts, how far up the decimation ladder this
/// device is actually reliable.
///
/// `validate_iq_mode` permits any sample rate up to `receiver_clock / 1.5`,
/// which for the hardcoded 92.16 MHz clock is 61.44 MHz. Field testing
/// on a V6 ECO found that ceiling is optimistic: the USB link, not the
/// clock, runs out first. This test turns that into a table so the
/// honest limit can be documented and enforced instead of guessed at.
#[tokio::test]
#[ignore = "requires an attached Spectran V6; several minutes"]
async fn sample_rate_ladder_characterisation() {
    let _guard = device_lock().await;

    let trials = env_usize("AARONIA_SDK_TRIALS", 5);
    // The documented ladder, from the top permitted by the 92.16 MHz
    // clock down to a rung that is known-good.
    let rungs = [
        61.44e6, 49.152e6, 30.72e6, 24.576e6, 15.36e6, 10.0e6, 7.68e6, 3.84e6,
    ];

    println!();
    println!("  requested (MHz)   device reports (MS/s)   trials   ok   failed   first error");
    println!("  ---------------   ---------------------   ------   --   ------   -----------");

    let mut table = Vec::new();

    for rate in rungs {
        let mut ok = 0usize;
        let mut first_err: Option<String> = None;
        let mut reported = 0.0f64;

        for _ in 0..trials {
            match try_capture(rate, 262_144).await {
                Ok((_, rate)) => {
                    ok += 1;
                    reported = rate;
                }
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
            // Let the device settle between trials; a wedged USB
            // endpoint otherwise poisons the next attempt.
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        let failed = trials - ok;
        println!(
            "  {:>15.3}   {:>21.3}   {:>6}   {:>2}   {:>6}   {}",
            rate / 1e6,
            reported / 1e6,
            trials,
            ok,
            failed,
            first_err.as_deref().unwrap_or("-")
        );
        table.push((rate, ok, trials));
    }

    println!();
    let highest_reliable = table
        .iter()
        .filter(|(_, ok, trials)| ok == trials)
        .map(|(rate, _, _)| *rate)
        .fold(0.0f64, f64::max);
    println!(
        "  highest fully-reliable requested sample rate on this unit: {:.3} MHz",
        highest_reliable / 1e6
    );

    assert!(
        highest_reliable > 0.0,
        "no sample rate succeeded on every trial — the device is not usable at any rung"
    );
}

/// A sample rate past what the receiver clock permits must be refused up front,
/// with a message that says so. The failure this guards against is the
/// SDK reporting an over-wide request as "No Spectran V6 devices found",
/// which sends the user hunting for a cabling fault that isn't there.
#[tokio::test]
#[ignore = "requires an attached Spectran V6"]
async fn oversized_sample_rate_is_refused_with_an_honest_message() {
    let _guard = device_lock().await;

    // 92.16 MHz / 1.5 = 61.44 MHz is the ceiling; ask for well past it.
    let err = AaroniaSource::new(sdk_config(120e6))
        .await
        .err()
        .expect("a rate above the clock ceiling must be refused");
    let msg = err.to_string().to_lowercase();
    println!("error: {msg}");

    assert!(
        !msg.contains("no spectran v6 devices found"),
        "an over-wide rate must not be reported as a missing device: {msg}"
    );
    assert!(
        // Broad on purpose: the refusal may come from this crate's own
        // `validate_iq_mode` ("sample rate ... exceeds ...") or, if a
        // vendor config write fails first, from the SDK, which still
        // speaks of "span". Either must name the quantity, not the device.
        msg.contains("rate") || msg.contains("span") || msg.contains("bandwidth"),
        "the error should name the sample rate as the problem, got: {msg}"
    );
}

// ---------------------------------------------------------------------
// F. Streaming correctness
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires an attached Spectran V6"]
async fn observed_sample_rate_matches_the_request() {
    let _guard = device_lock().await;

    let rate = 15.36e6;
    let mut source = AaroniaSource::new(sdk_config(rate))
        .await
        .expect("device must open");
    source.start_streaming().await.expect("start");

    let mut buf: Vec<Complex32> = Vec::new();
    // Prime for a fixed interval rather than one read. The ECO's
    // iqreceiver pipeline delivers ~40% of its rate for the first ~5 s
    // after starting, then settles at ~98%; a 5 s window that begins
    // at start measures the transient, not the stream. Measured on a
    // V6 ECO: 9.65 MS/s over the first 5 s, 22.6 MS/s over the next 25.
    let warmup = Instant::now();
    while warmup.elapsed() < Duration::from_secs(8) {
        let _ = source.read_samples(&mut buf, 262_144).await;
        buf.clear();
    }

    let start = Instant::now();
    let mut total = 0usize;
    while start.elapsed() < Duration::from_secs(10) {
        total += source.read_samples(&mut buf, 262_144).await.unwrap_or(0);
        buf.clear();
    }
    let measured = total as f64 / start.elapsed().as_secs_f64();
    // What the device's own packets say it is sending — the number the
    // stream must be held to. `sample_rate_hz` reports it once a
    // packet has been read; before this existed it echoed the request.
    let reported = source.sample_rate_hz();
    let _ = source.stop_streaming().await;

    println!(
        "requested {:.3} MS/s, device reports {:.3} MS/s, measured {:.3} MS/s over {} samples \
         (reported/requested = {:.3})",
        rate / 1e6,
        reported / 1e6,
        measured / 1e6,
        total,
        reported / rate
    );
    assert!(
        (measured - reported).abs() / reported < 0.05,
        "sustained {measured:.0} S/s is more than 5% from the {reported:.0} S/s the device reports: \
         samples are being lost between the SDK and the caller"
    );
}

#[tokio::test]
#[ignore = "requires an attached Spectran V6; 60 s by default"]
async fn long_run_is_stable_and_reports_its_drops() {
    let _guard = device_lock().await;

    let secs = env_f64("AARONIA_SDK_SOAK_SECS", 60.0);
    let mut source = AaroniaSource::new(sdk_config(15.36e6))
        .await
        .expect("device must open");
    source.start_streaming().await.expect("start");

    let mut buf: Vec<Complex32> = Vec::new();

    // Run past the startup transient before counting anything: the
    // ECO's iqreceiver delivers ~40% of rate for ~5 s after start and
    // flags one discontinuity as it settles. Counting from t=0 measured
    // that every time and called it instability. Baseline the counters
    // once the stream is steady, then hold the next `secs` to zero.
    let warmup = Instant::now();
    while warmup.elapsed() < Duration::from_secs(8) {
        let _ = source.read_samples(&mut buf, 262_144).await;
        buf.clear();
    }
    let drops_at_start = source.cumulative_drops();
    let _ = source.take_overrun();

    let mut total = 0usize;
    let mut overruns = 0usize;
    let start = Instant::now();
    while start.elapsed().as_secs_f64() < secs {
        total += source.read_samples(&mut buf, 262_144).await.unwrap_or(0);
        buf.clear();
        if source.take_overrun() {
            overruns += 1;
        }
    }

    let drops = source.cumulative_drops() - drops_at_start;
    let reported = source.sample_rate_hz();
    let elapsed = start.elapsed().as_secs_f64();
    let _ = source.stop_streaming().await;

    let measured = total as f64 / elapsed;
    println!(
        "soak: {total} samples in {elapsed:.1}s = {:.3} MS/s against {:.3} reported \
         ({:.1}%), {drops} gap events, {overruns} overruns, {drops_at_start} gaps during warm-up",
        measured / 1e6,
        reported / 1e6,
        100.0 * measured / reported
    );
    assert!(total > 0, "soak produced no samples at all");
    assert_eq!(drops, 0, "a steady-state soak should see no timestamp gaps");
    assert_eq!(
        overruns, 0,
        "a steady-state soak should see no overrun-flagged packets"
    );
    assert!(
        (measured - reported).abs() / reported < 0.05,
        "steady-state delivery {measured:.0} S/s is more than 5% from the reported {reported:.0} S/s"
    );
}

// ---------------------------------------------------------------------
// G. Cross-API parity — the paths the audit found untested
// ---------------------------------------------------------------------

/// The Seify backend had no tests of any kind. `sdk=true` must reach the
/// native SDK, not fall through to HTTP.
///
/// A plain `#[test]`, deliberately: `AaroniaSeifyDevice` owns a tokio
/// runtime, and dropping one inside an async context is a tokio panic
/// ("Cannot drop a runtime in a context where blocking is not
/// allowed"). The first run of this test on hardware opened the device
/// fine and then died in teardown for exactly that reason.
#[cfg(feature = "seify")]
#[test]
#[ignore = "requires an attached Spectran V6; build with --features seify,native-sdk"]
fn seify_backend_reaches_the_native_sdk() {
    use seify::{Args, DeviceInfo};
    let _guard = device_lock_blocking();

    let mut args = Args::new();
    args.set("sdk", "true");
    args.set("freq", center_hz().to_string());
    args.set("rate", "15360000");

    let dev = sdr_aaronia_rs::seify_impl::AaroniaSeifyDevice::from_args(&args)
        .expect("seify device must open through the native SDK");

    println!("seify device id = {:?}", dev.id());
}

/// The C ABI is what the Python bindings and the SoapySDR plugin both
/// ride on. None of its tests touched the SDK source type.
///
/// Note what this test documents as much as what it asserts: the C ABI
/// exposes no `force_source_type`. Leaving both `http_source` and
/// `file_source` unset is the *only* way to reach the native SDK from
/// C, and it works by auto-detection — which silently falls back to
/// localhost HTTP when the SDK is not installed. The Rust API's
/// `force_native_sdk()` has no C equivalent.
#[cfg(feature = "ffi")]
#[test]
#[ignore = "requires an attached Spectran V6"]
fn c_api_reaches_the_native_sdk_by_autodetection() {
    let _guard = device_lock_blocking();

    unsafe {
        let builder = sdr_aaronia_rs::aaronia_source_builder_new();
        assert!(!builder.is_null(), "builder allocation");

        sdr_aaronia_rs::aaronia_source_builder_center_frequency(builder, center_hz());
        sdr_aaronia_rs::aaronia_source_builder_span_frequency(builder, 15.36e6);
        // Deliberately no http_source/file_source: that is what selects
        // auto-detection, and hence the SDK.

        let source = sdr_aaronia_rs::aaronia_source_build(builder);
        assert!(
            !source.is_null(),
            "native-SDK source must build over the C ABI when the SDK is installed"
        );

        let info = sdr_aaronia_rs::aaronia_source_get_source_info(source);
        assert!(!info.is_null(), "source info must be returned");
        // `CAaroniaSourceType` is a bare `#[repr(C)]` enum with no
        // derives, so match on it rather than formatting it.
        let is_native = matches!(
            (*info).source_type,
            sdr_aaronia_rs::CAaroniaSourceType::NativeSdk
        );
        println!("C ABI reached the native SDK: {is_native}");
        assert!(
            is_native,
            "auto-detection must land on the native SDK, not fall back to HTTP"
        );

        sdr_aaronia_rs::aaronia_source_info_free(info);
        sdr_aaronia_rs::aaronia_source_free(source);
    }
}
