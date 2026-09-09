# Synchronising two or more receivers

The short version: **the SPECTRAN V6 is a shared-reference machine, not a
commanded-time one.** You cannot tell it to start capturing at a stated
instant. You can lock several devices to one reference, timestamp
everything they produce, and align the streams afterwards. That is
enough for RDF, TDOA and coherent multi-channel work; it is not enough
for the UHD-style `set_start_time()` pattern, and no amount of API
wrapping will make it so.

## What the hardware offers

| Primitive | Where |
| --- | --- |
| Common frequency reference (10 MHz, PPS, GPS) | `device/sclksource` — `set_clock_source()` |
| GPS discipline and time of day | `device/gpsmode` — `set_gps_mode()`, `gps_time_ns()` |
| Per-packet hardware timestamps | `AARTSAAPI_Packet::startTime`/`endTime` — `last_timestamp_ns()` |
| The device's own stream clock | `AARTSAAPI_GetMasterStreamTime` — `master_stream_time_ns()` |
| Two phase-coherent RX inputs (full V6) | `device/receiverchannel` — `read_samples_dual()` |

A V6 ECO with no GPS antenna accepts six clock sources: `Consumer`,
`Oscillator`, `PPS`, `10MHz`, `Oscillator Provider` and `PPS Provider`.
The `Provider` variants make that device the reference for others rather
than a consumer of one. `GPS` and `GPS Provider` appear in the tree but
are masked out when no antenna is attached, and `clock_sources()`
filters them for that reason — the list reports what the device will
accept, not what it can spell.

## What it does not offer

The vendor API is 34 functions. There is no set-time, no arm-at-time and
no trigger among them: the closest thing to time control is
`AARTSAAPI_GetMasterStreamTime`, which reads. Nothing writes a clock,
and nothing schedules a capture.

Transmission is the exception, and it proves the rule — a `TxBurst`
carries `startTime`/`endTime` and the device schedules it. The
capability exists in the FPGA; the API simply does not expose it for
receive.

So a two-receiver capture cannot be started simultaneously by command.
Both devices are started as promptly as software allows, and the offset
between them is measured rather than commanded.

## The recipe

1. **Discipline every device from one reference.** One device (or an
   external source) provides; the rest consume:

   ```rust,no_run
   # async fn discipline() -> sdr_aaronia_rs::Result<()> {
   use sdr_aaronia_rs::{SpectranConfig, SpectranSource};

   let mut provider =
       SpectranSource::new(SpectranConfig::from_http("http://ref.local:54664")).await?;
   let mut consumer =
       SpectranSource::new(SpectranConfig::from_http("http://sat.local:54664")).await?;

   provider.set_clock_source("PPS Provider").await?;
   consumer.set_clock_source("PPS").await?;
   # Ok(())
   # }
   ```

   Confirm by reading back — `set_clock_source` already does, and errors
   if the device did not take it. This is what makes the sample clocks
   agree in *rate*. It does not make them agree in *phase*.

2. **Start the streams.** In whatever order; the offset between them is
   not controlled and does not need to be.

3. **Timestamp everything.** Each packet carries device time.
   `last_timestamp_ns()` reports the most recent one, and every
   SoapySDR buffer comes back flagged `SOAPY_SDR_HAS_TIME`.

4. **Align afterwards.** Compute the offset between the streams from
   their timestamps, then refine it by cross-correlating a common
   signal. The timestamps get you to within the resolution below; the
   correlation gets you the rest of the way, and is what actually
   determines the answer in TDOA work.

For two channels on a *single* full V6, none of this applies: `Rx1` and
`Rx2` share one tuner and one converter, arrive interleaved in one
packet, and are sample-aligned by construction. Read them with
`read_samples_dual()` — see [SDKSPEC.md](SDKSPEC.md#receiver-channel-selection-and-dual-channel-capture).

## The resolution floor

The device reports both its GPS time and its master stream time as
`float64` seconds since the epoch. At present-day epoch values an `f64`
steps about **238 ns**, so those two clocks are quantised at roughly
that — before any conversion this crate performs, and not something it
can improve on.

For TDOA, 240 ns of timing uncertainty is about **70 m** of ranging
uncertainty. That is the floor for *timestamp-based* alignment. It is
not the floor for the technique: cross-correlating a common signal
resolves far below one sample period, and the timestamps only have to be
good enough to identify which samples to correlate. Build the geometry
on the correlation, and use the timestamps to get there.

The per-packet stream timestamps are the better of the two clocks for
this. GPS time is for disciplining and for wall-clock labelling.

## Status

Hardware-unverified. The clock-source writes are exercised against a
single V6 ECO; nothing here has been run across two devices, and the
crate has never seen a full V6. See
[VERIFICATION.md](VERIFICATION.md).

## Related

- [SDKSPEC.md](SDKSPEC.md) — the native SDK surface, including the
  packet layout and dual-channel capture.
- [USAGE.md](USAGE.md) — worked examples for each part of the API.
- [VERIFICATION.md](VERIFICATION.md) — what has been tested against
  hardware.
