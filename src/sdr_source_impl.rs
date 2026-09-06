//! [`SdrSource`] implementation for the Aaronia source family.
//!
//! Wraps the existing async [`AaroniaSourceBuilder`] in a synchronous
//! [`SdrSource::start`] entry point: spawns a dedicated thread with
//! its own tokio runtime, drives the builder + read-samples loop
//! inside that runtime, and bridges async-yielded samples to a sync
//! crossbeam channel. The orchestrator consumes the channel without
//! caring that the underlying transport (HTTP, file, native SDK) is
//! async.
//!
//! Naming: the existing [`crate::AaroniaSource`] is the unified
//! async-API source. The new [`AaroniaSdrSource`] here is the
//! `SdrSource`-trait facade that constructs and drives one. They
//! live in the same crate but serve different layers.

use crate::AaroniaSourceBuilder;
use crate::sdr_source::{
    DwellAdvice, DwellController, IqPacket, SdrError, SdrHandle, SdrSource, SourceConfig,
    freq_key_khz,
};
use crate::unified_source::SourceType;
use crate::{Error, Result};
use crossbeam_channel as channel;
use num_complex::Complex32;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Picks one of the three Aaronia transport paths. Mirrors the
/// orchestrator's `aaronia http` / `aaronia file` / `aaronia sdk`
/// subcommands one-to-one.
#[derive(Debug, Clone)]
pub enum AaroniaBackend {
    /// HTTP server-block endpoint of a running RTSA-Suite.
    Http(String),
    /// `.rtsa` capture file replayed through the unified source.
    File(PathBuf),
    /// Direct USB via the native AARTSAAPI SDK. `serial` disambiguates
    /// when multiple Spectran V6 / V6 ECO devices are attached.
    Sdk { serial: Option<String> },
}

/// `SdrSource`-trait facade for Aaronia. Pair with an
/// [`AaroniaBackend`], a centre frequency, and a reference level; the
/// trait machinery handles the rest.
///
/// **Hopping**: when [`SourceConfig::channels_hz`] is non-empty, the
/// HTTP and native-SDK backends cycle through the channel list using
/// [`DwellController`] for per-hop pacing (same logic as the USRP
/// backend). File backends ignore hopping — RTSA capture files carry
/// a single centre frequency in their metadata.
pub struct AaroniaSdrSource {
    /// Transport selection: HTTP endpoint, capture file, or native SDK.
    pub backend: AaroniaBackend,
    /// Initial center frequency, in Hz.
    pub center_frequency_hz: f64,
    /// Reference level, in dBm.
    pub reference_level_dbm: f64,
    /// Samples requested per `read_samples` call. Larger blocks
    /// amortise HTTP chunk decode overhead; smaller blocks reduce
    /// latency. The orchestrator's default is 65 536.
    pub block_size: usize,
    /// HTTP `/stream` wire format passthrough for the
    /// [`AaroniaBackend::Http`] backend; `None` uses the library default
    /// (binary Float32). Ignored by the file and native-SDK backends.
    /// Exists so orchestrators can select e.g. Int16 (half the network
    /// bandwidth of Float32) without reaching into the unified source
    /// themselves.
    pub stream_format: Option<crate::http_streaming::StreamFormat>,
}

/// Tunable: how long to actively drain the read buffer after a
/// retune before we trust subsequent reads. RTSA HTTP takes ~50–100
/// ms to apply `configure_capture` server-side; the native SDK
/// config-write is faster but the IQ pipeline still flushes a few
/// packets at the old frequency. We pump `read_samples` in a tight
/// loop during this window and discard everything we get — that
/// keeps stale old-channel data from being tagged with the new
/// channel frequency (a real false-positive driver: a transmitter
/// on channel A would otherwise register as a detection on
/// channel B for the first dwell after a hop).
const RETUNE_SETTLE: Duration = Duration::from_millis(75);

/// Number of consecutive empty `read_samples` calls before assuming
/// the source is wedged or at EOF (file backend) and exiting the
/// current hop / pump.
const EMPTY_READ_BAILOUT: u32 = 16;

/// Number of consecutive `read_samples` errors before we give up on
/// the source entirely and exit the capture thread. A single error
/// might be transient (network glitch, SDK reconfigure mid-stream);
/// persistent errors mean the source is dead and the orchestrator
/// should see EOF rather than be silently fed an empty channel.
const READ_ERROR_BAILOUT: u32 = 5;

impl SdrSource for AaroniaSdrSource {
    fn start(
        self: Box<Self>,
        config: SourceConfig,
        advice: Arc<dyn DwellAdvice>,
    ) -> Result<SdrHandle, SdrError> {
        let (tx, receiver) = channel::bounded::<IqPacket>(1024);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_thread = stop_flag.clone();

        let AaroniaSdrSource {
            backend,
            center_frequency_hz,
            reference_level_dbm,
            block_size,
            stream_format,
        } = *self;
        let block_size = block_size.max(1024);
        let span_hz = config.sample_rate_hz;
        let channels_hz = config.channels_hz.clone();
        let dwell_ctrl = DwellController {
            min: config.dwell_min,
            max: config.dwell_max,
            extension: config.dwell_extension,
        };

        let capture_thread =
            thread::spawn(move || {
                let panic_res =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                        if let Err(e) = (move || -> Result<(), Error> {
                            let runtime = tokio::runtime::Builder::new_multi_thread()
                                .enable_all()
                                .thread_name("sdr-aaronia-pump")
                                .build()?;

                            runtime.block_on(async move {
                    let mut builder = AaroniaSourceBuilder::new();
                    builder
                        .center_frequency(center_frequency_hz)
                        .span_frequency(span_hz)
                        .reference_level(reference_level_dbm);

                    match &backend {
                        AaroniaBackend::Http(url) => {
                            builder.http_source(url.clone());
                            if let Some(format) = stream_format {
                                builder.stream_format(format);
                                info!("Aaronia HTTP source: {} (format: {})", url, format.as_str());
                            } else {
                                info!("Aaronia HTTP source: {}", url);
                            }
                        }
                        AaroniaBackend::File(path) => {
                            builder.file_source(path.to_string_lossy().as_ref());
                            info!("Aaronia file source: {}", path.display());
                        }
                        AaroniaBackend::Sdk { serial } => {
                            builder.force_source_type(SourceType::NativeSdk);
                            if let Some(s) = serial {
                                builder.device_serial(s.clone());
                                info!("Aaronia native SDK source (serial = {})", s);
                            } else {
                                info!("Aaronia native SDK source (auto-select first device)");
                            }
                        }
                    }

                    let mut source = builder.build().await.map_err(|e| {
                        Error::Config(format!("AaroniaSourceBuilder::build failed: {e}"))
                    })?;
                    let source_info = source.get_source_info();
                    info!("Aaronia source: {:?}", source_info);
                    // For file backends the RTSA metadata is authoritative
                    // for the center frequency — the caller passes a 0.0
                    // placeholder because it can't know the file's tuning up
                    // front. `build()` resolves it into the config, so prefer
                    // the value from `source_info`; fall back to the caller's
                    // value for live backends where the two already agree.
                    let effective_center_hz = if source_info.center_frequency > 0.0 {
                        source_info.center_frequency
                    } else {
                        center_frequency_hz
                    };

                    source
                        .start_streaming()
                        .await
                        .map_err(|e| Error::Config(format!("start_streaming failed: {e}")))?;

                    // File backends always run single-channel: RTSA files
                    // carry one frequency in their metadata, so hopping is
                    // meaningless and would just spew warnings from
                    // `set_center_frequency`. Other backends hop if and
                    // only if the orchestrator handed us a non-empty
                    // channel list (USRP-style hop config).
                    //
                    // Retuning (both hop-mode and the WS-B live-view
                    // override) always goes through `set_center_frequency`,
                    // which on HTTP backends is `configure_capture` on the
                    // free `/control` endpoint — never the licensed
                    // `/remoteconfig` write path. We deliberately don't
                    // probe for the "Remote Config" license before hopping:
                    // that probe itself reads the `/remoteconfig` tree,
                    // which is the endpoint we're avoiding, and gating
                    // hopping on it meant a server the probe couldn't
                    // positively confirm fell back to single-channel even
                    // though `/control` retunes work. `probe_remote_config_license`
                    // is still available on `AaroniaSource` for callers who
                    // want it for their own purposes; this orchestrator no
                    // longer calls it.
                    let hopping =
                        !channels_hz.is_empty() && !matches!(backend, AaroniaBackend::File(_));

                    let (pool_tx, pool_rx) = channel::bounded::<Vec<Complex32>>(256);
                    for _ in 0..256 {
                        let _ = pool_tx.send(Vec::with_capacity(block_size));
                    }

                    if hopping {
                        info!(
                            "Aaronia hop mode: {} channels, dwell {:?}-{:?} (adaptive={})",
                            channels_hz.len(),
                            dwell_ctrl.min,
                            dwell_ctrl.max,
                            dwell_ctrl.is_adaptive(),
                        );
                        hop_pump(
                            &mut source,
                            &channels_hz,
                            &dwell_ctrl,
                            advice.as_ref(),
                            &tx,
                            &stop_thread,
                            block_size,
                            &pool_rx,
                            &pool_tx,
                        )
                        .await
                        .map_err(|e| Error::Config(e.to_string()))?;
                    } else {
                        single_channel_pump(
                            &mut source,
                            effective_center_hz,
                            advice.as_ref(),
                            &tx,
                            &stop_thread,
                            block_size,
                            &pool_rx,
                            &pool_tx,
                        )
                        .await
                        .map_err(|e| Error::Config(e.to_string()))?;
                    }

                    let _ = source.stop_streaming().await;
                    Ok::<_, Error>(())
                })?;
                            Ok(())
                        })() {
                            tracing::error!("[aaronia] Capture thread failed: {:?}", e);
                        }
                    }));
                if let Err(e) = panic_res {
                    tracing::error!("[aaronia] Capture thread panicked: {:?}", e);
                }
            });

        let stop_handle = stop_flag.clone();
        let stop = Box::new(move || stop_handle.store(true, Ordering::SeqCst));
        let wait = Box::new(move || {
            if let Err(e) = capture_thread.join() {
                tracing::error!("[aaronia] capture thread join failed: {:?}", e);
            }
        });
        Ok(SdrHandle {
            receiver,
            stop,
            wait,
        })
    }
}

/// Pump samples on a single centre frequency. Pre-A37 behaviour — used
/// for file replay and for direct callers of `AaroniaSdrSource` that
/// build with an empty `channels_hz` list (including hop configs that
/// fell back here after `probe_remote_config_license` failed).
///
/// **Live-view override**: while [`DwellAdvice::channel_override`]
/// returns `Some`, retunes there and holds, independent of the hop
/// license gate above — a single park-and-hold retune (as opposed to
/// rapid mid-stream hopping) is the light case `configure_capture` on
/// `/control` always handles, licensed or not. Returns to
/// `center_frequency_hz` once the override clears. On a file-backed
/// source `set_center_frequency` is already a documented no-op (RTSA
/// files carry their own frequency — Phase A's wideband replay covers
/// the requested channel without any retune), so this degrades safely
/// there.
#[allow(clippy::too_many_arguments)]
async fn single_channel_pump(
    source: &mut crate::AaroniaSource,
    center_frequency_hz: f64,
    advice: &dyn DwellAdvice,
    tx: &channel::Sender<IqPacket>,
    stop_thread: &AtomicBool,
    block_size: usize,
    pool_rx: &channel::Receiver<Vec<Complex32>>,
    pool_tx: &channel::Sender<Vec<Complex32>>,
) -> Result<()> {
    let mut empty_reads = 0u32;
    let mut current_freq = center_frequency_hz;
    while !stop_thread.load(Ordering::SeqCst) {
        let target = advice.channel_override().unwrap_or(center_frequency_hz);
        if (target - current_freq).abs() > 1.0 {
            if let Err(e) = source.set_center_frequency(target).await {
                warn!("Aaronia retune to {:.3} MHz failed: {e}", target / 1e6);
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            current_freq = target;
            drain_during_settle(source, block_size, RETUNE_SETTLE).await;
        }

        let mut raw_buffer = pool_rx
            .try_recv()
            .unwrap_or_else(|_| Vec::with_capacity(block_size));
        raw_buffer.clear();

        let n = match source.read_samples(&mut raw_buffer, block_size).await {
            Ok(v) => v,
            Err(e) => {
                warn!("Aaronia read_samples error: {e} — stopping");
                break;
            }
        };
        if n == 0 {
            empty_reads += 1;
            if empty_reads >= EMPTY_READ_BAILOUT {
                info!("Aaronia source returned {EMPTY_READ_BAILOUT} empty reads — assuming EOF");
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
            continue;
        }
        empty_reads = 0;
        // The rate is read per packet: on HTTP the rate the device actually
        // streams is only known once packets flow, and may not be the one
        // asked for (it snaps to the decimation ladder).
        let pkt = IqPacket {
            samples: crate::sdr_source::PooledIqBuffer::new_pooled(raw_buffer, pool_tx.clone()),
            center_frequency_hz: current_freq,
            sample_rate_hz: source.sample_rate_hz() as f32,
            overrun: source.take_overrun(),
        };
        if tx.send(pkt).is_err() {
            break; // consumer dropped
        }
    }
    Ok(())
}

/// Cycle through `channels_hz` indefinitely, retuning the source at
/// each hop and pumping samples until the per-hop deadline elapses or
/// the consumer signals stop. Mirrors `UsrpSource`'s hop logic, with
/// per-retune settle (during which the read buffer is actively
/// drained) to ride out the RTSA's apply-config latency.
///
/// Persistent retune or read failures (more than
/// [`READ_ERROR_BAILOUT`] in a row) exit the thread instead of
/// spinning the outer loop — a disconnected SDR should surface as
/// EOF to the orchestrator, not as a tight retune retry loop.
#[allow(clippy::too_many_arguments)]
async fn hop_pump(
    source: &mut crate::AaroniaSource,
    channels_hz: &[f64],
    dwell_ctrl: &DwellController,
    advice: &dyn DwellAdvice,
    tx: &channel::Sender<IqPacket>,
    stop_thread: &AtomicBool,
    block_size: usize,
    pool_rx: &channel::Receiver<Vec<Complex32>>,
    pool_tx: &channel::Sender<Vec<Complex32>>,
) -> Result<()> {
    let mut channel_idx = 0usize;
    // Track the currently-tuned channel so we can short-circuit the
    // retune+settle when the next hop is to the same frequency
    // (degenerate case for one-channel hop lists like `--region
    // test`). Initialised to NaN so the first hop always retunes.
    let mut current_channel = f64::NAN;
    let mut consecutive_retune_failures = 0u32;
    // `read_errors` lives across hops so a dead source eventually
    // hits the bailout. Resetting it per-hop (the obvious-looking
    // scoping) would mean a permanently broken source spins
    // forever hopping channels with one error per hop. `empty_reads`
    // by contrast is per-hop on purpose: a quiet channel isn't a
    // dead source, and hopping is the cure.
    let mut read_errors = 0u32;
    while !stop_thread.load(Ordering::SeqCst) {
        // A live `/video` viewer wants a specific channel: park there,
        // bypassing the hop list, until the override changes or clears.
        // `channel_idx` only advances on a genuine hop below, so hopping
        // resumes exactly where it left off once the viewer disconnects.
        let override_freq = advice.channel_override();
        let channel = override_freq.unwrap_or_else(|| {
            let c = channels_hz[channel_idx];
            channel_idx = (channel_idx + 1) % channels_hz.len();
            c
        });

        // Short-circuit retune+settle when the hop target equals
        // the current channel — otherwise a one-channel hop list
        // would burn ~75 ms per dwell cycle on a no-op tune.
        if channel != current_channel {
            if let Err(e) = source.set_center_frequency(channel).await {
                warn!("Aaronia retune to {:.3} MHz failed: {e}", channel / 1e6);
                consecutive_retune_failures += 1;
                if consecutive_retune_failures >= READ_ERROR_BAILOUT {
                    return Err(Error::Config(format!(
                        "{READ_ERROR_BAILOUT} consecutive retune failures — source likely dead"
                    )));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            consecutive_retune_failures = 0;
            current_channel = channel;
            // Actively drain the read buffer during the settle so
            // stale old-channel samples don't get tagged with the
            // new channel. Critical to avoid false detections:
            // pre-fix a transmitter on channel A could register
            // as a detection on channel B for the first dwell
            // after a hop.
            drain_during_settle(source, block_size, RETUNE_SETTLE).await;
        }

        let hop_start = Instant::now();
        let freq_key = freq_key_khz(channel);
        let mut deadline = dwell_ctrl.deadline(hop_start, advice.latest_signal_at(freq_key));
        let mut empty_reads = 0u32;

        loop {
            if stop_thread.load(Ordering::SeqCst) {
                break;
            }
            let still_overridden = advice.channel_override() == Some(channel);
            if override_freq.is_some() {
                // Parked here for live view: hold regardless of the dwell
                // deadline, but break out the moment the viewer's channel
                // changes or they disconnect so the outer loop can retune
                // or resume hopping.
                if !still_overridden {
                    break;
                }
            } else if still_overridden {
                // A viewer just requested this exact channel while we were
                // still mid-hop-dwell here — break out now (rather than
                // waiting for the normal dwell deadline) so the next outer
                // iteration parks on it promptly.
                break;
            } else if Instant::now() >= deadline {
                break;
            }

            let mut raw_buffer = pool_rx
                .try_recv()
                .unwrap_or_else(|_| Vec::with_capacity(block_size));
            raw_buffer.clear();

            // Bound the read by the dwell deadline. `read_samples` waits
            // until the block fills or `read_timeout` (30 s by default)
            // expires — orders of magnitude longer than a dwell. A
            // stalled server would hold the pump here and starve every
            // remaining hop; with auto-reconnect enabled a stream
            // reconnecting through its backoff does the same, since the
            // channel stays open instead of closing. Returning whatever
            // arrived by the deadline keeps the hop cadence intact, and
            // the `n == 0` branch below already handles an empty read.
            let remaining = deadline.saturating_duration_since(Instant::now());
            let n = match source
                .read_samples_deadline(&mut raw_buffer, block_size, remaining)
                .await
            {
                Ok(v) => {
                    read_errors = 0;
                    v
                }
                // A dwell that ends before any samples arrive is an empty
                // read, not a broken source; the HTTP path reports it as a
                // timeout rather than `Ok(0)`.
                Err(crate::Error::Io(ref e)) if e.kind() == std::io::ErrorKind::TimedOut => 0,
                Err(e) => {
                    read_errors += 1;
                    if read_errors >= READ_ERROR_BAILOUT {
                        return Err(Error::Config(format!(
                            "{READ_ERROR_BAILOUT} consecutive read errors — source likely dead: {e}"
                        )));
                    }
                    warn!("Aaronia read error during hop: {e}");
                    break;
                }
            };
            if n == 0 {
                empty_reads += 1;
                if empty_reads >= EMPTY_READ_BAILOUT {
                    warn!(
                        "{EMPTY_READ_BAILOUT} empty reads on channel {:.3} MHz — moving on",
                        channel / 1e6
                    );
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
                continue;
            }
            empty_reads = 0;
            let pkt = IqPacket {
                samples: crate::sdr_source::PooledIqBuffer::new_pooled(raw_buffer, pool_tx.clone()),
                center_frequency_hz: channel,
                sample_rate_hz: source.sample_rate_hz() as f32,
                overrun: source.take_overrun(),
            };
            if tx.send(pkt).is_err() {
                return Ok(()); // consumer dropped
            }
            // Re-evaluate the deadline against the freshest signal
            // observation if adaptive dwell is enabled. Quiet
            // channels stay at the minimum; hot channels keep the
            // deadline pushed out under EMA-style extension.
            if dwell_ctrl.is_adaptive() {
                deadline = dwell_ctrl.deadline(hop_start, advice.latest_signal_at(freq_key));
            }
        }
    }
    Ok(())
}

/// Drain stale samples from the source during the post-retune settle
/// window. Reads in a tight loop and discards everything until the
/// window elapses or `read_samples` errors out (in which case the
/// caller's main loop surfaces the error on the next read).
async fn drain_during_settle(
    source: &mut crate::AaroniaSource,
    block_size: usize,
    settle: Duration,
) {
    let deadline = Instant::now() + settle;
    let mut drain_buf = Vec::with_capacity(block_size);
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        drain_buf.clear();
        match tokio::time::timeout(remaining, source.read_samples(&mut drain_buf, block_size)).await
        {
            Ok(Ok(_samples)) => continue, // discard old-channel data
            Ok(Err(_)) | Err(_) => break,
        }
    }
}
