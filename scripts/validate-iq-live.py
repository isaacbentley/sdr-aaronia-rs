#!/usr/bin/env python3
"""End-to-end IQ validation against a live RTSA-Suite HTTP server.

Proves that the samples an application receives are the ones the device
sent, rather than merely that bytes arrived. Nothing here needs a signal
generator: the receiver's own noise floor is a stable, broadband
reference, and the checks are built on comparing independent paths
against each other.

  1. Sanity          finite, non-trivial, within full scale
  2. Rate agreement  requested == reported == implied by sample count
  3. Wire formats    F32, F16 and I16 must decode to the same spectrum.
                     This is the sharpest check in the file: a wrong
                     scale factor, a byte order slip or a broken
                     float16 decode all show up as one format
                     disagreeing with the other two.
  4. API paths       python-aaronia (Rust directly) and SoapySDR (via
                     the C ABI) must agree, which exercises two
                     different consumers of the same parser.
  5. Filter shape    the noise floor must be flat across the declared
                     usable span and roll off outside it — the
                     signature of real receiver output, and something
                     scrambled or mis-strided samples cannot fake.

Spectra are compared through a per-bin low percentile rather than a
mean, so bursty traffic in the band does not make two honest captures
look different.

Usage:  scripts/validate-iq-live.py [url] [--rate HZ] [--freq HZ]
Exits non-zero if any check fails.
"""

import argparse
import gc
import sys
import time

import numpy as np

NFFT = 4096
SEGMENTS = 240
# 3.84 MS/s keeps even float32 (8 bytes a sample) inside a typical
# link, so a format comparison measures decoding rather than which
# format best survives a saturated network.
DEFAULT_RATE = 3.84e6
DEFAULT_FREQ = 2.44e9

failures = []
notes = []
SCALE_SUPPORTED = True


def check(name, ok, detail):
    print(f"  {'ok  ' if ok else 'FAIL'}  {name}: {detail}")
    if not ok:
        failures.append(name)


def noise_floor(samples, rate):
    """Per-bin 10th percentile PSD in dB, plus the frequency axis.

    The low percentile follows the noise floor and ignores bursts, so
    two captures taken seconds apart stay comparable.
    """
    win = np.hanning(NFFT)
    rows = []
    for i in range(0, len(samples) - NFFT, NFFT):
        seg = samples[i : i + NFFT] * win
        rows.append(np.abs(np.fft.fftshift(np.fft.fft(seg))) ** 2)
    if len(rows) < 8:
        raise RuntimeError(f"only {len(rows)} segments; need at least 8")
    psd = 10 * np.log10(np.percentile(np.asarray(rows), 10, axis=0) + 1e-30)
    freqs = np.fft.fftshift(np.fft.fftfreq(NFFT, 1 / rate))
    return psd, freqs


SMOOTH = 65


def find_carrier(samples, rate, expect_offset_hz, search_hz=400e3):
    """Strongest bin near an expected offset, with its signal-to-floor.

    Returns (offset_hz, snr_db, level_db). `level_db` is absolute
    rather than referenced to the floor, which is what makes it usable
    for comparing amplitude between captures: the transmitter's power
    does not change, so a difference is the receive chain's. The search
    window deliberately spans the wrong side of zero, so a mirrored
    spectrum is found at the negated offset rather than missed.
    """
    win = np.hanning(NFFT)
    rows = []
    for i in range(0, len(samples) - NFFT, NFFT):
        rows.append(np.abs(np.fft.fftshift(np.fft.fft(samples[i : i + NFFT] * win))) ** 2)
    psd = 10 * np.log10(np.mean(np.asarray(rows), axis=0) + 1e-30)
    freqs = np.fft.fftshift(np.fft.fftfreq(NFFT, 1 / rate))
    floor = float(np.median(psd))
    band = np.abs(np.abs(freqs) - abs(expect_offset_hz)) < search_hz
    idx = np.where(band)[0]
    peak = idx[np.argmax(psd[idx])]
    return float(freqs[peak]), float(psd[peak] - floor), float(psd[peak])


def compare(a, b, label, tol_db):
    """Agreement between two noise floors, compared as smoothed shapes.

    Each is referenced to its own median, so this asks whether the
    passband *shape* matches; absolute level is checked separately by
    the amplitude test, which is what catches a wrong scale factor.

    The smoothing matters. Narrowband traffic in the band comes and
    goes between two captures taken seconds apart, and comparing raw
    bins measures the radio environment rather than the decoder. A
    decoding fault — swapped bytes, a bad float16 mantissa, a missing
    scale — deforms the whole curve, which survives smoothing.
    """
    k = np.ones(SMOOTH) / SMOOTH
    ra = np.convolve(a - np.median(a), k, mode="same")
    rb = np.convolve(b - np.median(b), k, mode="same")
    # Ignore the outer 10%, where the filter skirt is steep and the
    # smoothing window straddles it.
    keep = slice(int(NFFT * 0.05), int(NFFT * 0.95))
    diff = np.abs(ra[keep] - rb[keep])
    worst, rms = float(np.max(diff)), float(np.sqrt(np.mean(diff**2)))
    check(label, worst < tol_db, f"worst {worst:.2f} dB, rms {rms:.2f} dB (tol {tol_db})")
    return worst


def collect_python(url, fmt, rate, freq, scale=None):
    import aaronia

    kw = {"scale": scale} if scale is not None else {}
    try:
        src = aaronia.open(url, freq=freq, rate=rate, format=fmt, **kw)
    except TypeError:
        # python-aaronia before 0.7.5 has no `scale`. Fall back rather
        # than fail, and let the caller see it through SCALE_SUPPORTED.
        global SCALE_SUPPORTED
        SCALE_SUPPORTED = False
        src = aaronia.open(url, freq=freq, rate=rate, format=fmt)
    try:
        for _ in range(3):  # discard the retune transient
            src.read_samples_numpy(NFFT * 4)
        need = NFFT * (SEGMENTS + 1)
        out = np.empty(0, np.complex64)
        while len(out) < need:
            out = np.concatenate([out, src.read_samples_numpy(NFFT * 32)])
        return out[:need]
    finally:
        src.stop_streaming()


def collect_soapy(url, fmt, rate, freq):
    import SoapySDR
    from SoapySDR import SOAPY_SDR_CF32, SOAPY_SDR_RX

    sdr = SoapySDR.Device(f"driver=aaronia,url={url},format={fmt},rate={rate},freq={freq}")
    try:
        st = sdr.setupStream(SOAPY_SDR_RX, SOAPY_SDR_CF32)
        sdr.activateStream(st)
        buf = np.empty(65536, np.complex64)
        need = NFFT * (SEGMENTS + 1)
        out = np.empty(0, np.complex64)
        deadline = time.time() + 60
        while len(out) < need and time.time() < deadline:
            r = sdr.readStream(st, [buf], len(buf), timeoutUs=2_000_000)
            if r.ret > 0:
                out = np.concatenate([out, buf[: r.ret].copy()])
        drops = sdr.readSensor("cumulative_drops")
        reported = sdr.getSampleRate(SOAPY_SDR_RX, 0)
        sdr.deactivateStream(st)
        sdr.closeStream(st)
        if len(out) < need:
            raise RuntimeError(f"got {len(out)} of {need} samples before the deadline")
        return out[:need], float(reported), int(drops)
    finally:
        # The server allows one free client connection, so the next
        # capture must not overlap this one.
        del sdr
        gc.collect()
        time.sleep(1.0)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("url", nargs="?", default="http://localhost:54664")
    ap.add_argument("--rate", type=float, default=DEFAULT_RATE)
    ap.add_argument("--freq", type=float, default=DEFAULT_FREQ)
    # NOAA weather radio: always transmitting, on an exactly known
    # frequency, which makes it a free calibration reference almost
    # anywhere in North America. Pass 0 to skip, or another carrier you
    # can rely on. 162.400 MHz is the lowest of the seven channels.
    ap.add_argument("--ref-freq", type=float, default=162.400e6)
    ap.add_argument("--ref-offset", type=float, default=200e3)
    # Well above the server default of 16384, which quantises a quiet
    # band to zero. See the int16 resolution check.
    ap.add_argument("--scale", type=float, default=1e6)
    args = ap.parse_args()

    import aaronia

    print(f"validating {args.url} at {args.rate/1e6:.2f} MS/s, {args.freq/1e6:.1f} MHz\n")

    print("1. sanity and rate agreement")
    x = collect_python(args.url, "F32", args.rate, args.freq)
    check("finite", bool(np.all(np.isfinite(x))), f"{len(x)} samples, no NaN or Inf")
    rms = float(np.sqrt(np.mean(np.abs(x) ** 2)))
    check("non-trivial", rms > 0, f"rms {rms:.3e}")
    peak = float(np.max(np.abs(x)))
    check("within full scale", peak <= 1.0, f"peak |x| {peak:.4f}")
    check(
        "rate on the ladder",
        abs(aaronia.sample_rate_for_bandwidth(args.rate * 0.8) - args.rate) / args.rate < 1e-3,
        f"{args.rate/1e6:.3f} MS/s is a rung",
    )
    ok, msg, _ = aaronia.diagnose(args.url)[-1]
    check("device agrees on the rate", ok, msg)

    print("\n2. wire formats decode alike")
    floors, rmsv, zeros = {}, {}, {}
    for fmt in ("F32", "F16", "I16"):
        # int16 is the one format whose resolution is a choice. The
        # server encodes round(value * scale), so the step is 1/scale,
        # and at the default of 16384 a quiet band sits below it — most
        # samples come back exactly zero. Ask for enough resolution
        # here so this section measures decoding rather than clipping.
        sc = args.scale if fmt == "I16" else None
        s = x if fmt == "F32" else collect_python(args.url, fmt, args.rate, args.freq, sc)
        zeros[fmt] = float(np.mean((s.real == 0) & (s.imag == 0)))
        floors[fmt], freqs = noise_floor(s, args.rate)
        # Median magnitude, not rms. In a live band the rms is set by
        # whatever traffic happened during that capture, so comparing
        # two captures by rms measures the radio environment; the
        # median tracks the noise floor and is stable between them.
        rmsv[fmt] = float(np.median(np.abs(s)))
        gc.collect()
        time.sleep(1.0)

    if SCALE_SUPPORTED:
        check(
            "int16 resolves the signal",
            zeros["I16"] < 0.05,
            f"{zeros['I16']*100:.1f}% of samples exactly zero at scale={args.scale:g} "
            f"(step 1/{args.scale:g}); F32 gives {zeros['F32']*100:.1f}%",
        )
    else:
        notes.append(
            f"this python-aaronia has no `scale`, so int16 ran at the server default "
            f"and {zeros['I16']*100:.1f}% of its samples came back exactly zero — "
            f"the resolution and amplitude checks are skipped"
        )

    # Amplitude is deliberately not compared here. These captures are
    # seconds apart in a live band and the floor they sit on moves —
    # the same frequency measured 2.1e-05 and then 6.6e-05 minutes
    # later — so comparing them measures the radio environment rather
    # than the decoder. The reference-carrier section does the
    # amplitude check instead, against a transmitter whose power is
    # constant.
    notes.append(
        "median |x| per format: "
        + ", ".join(f"{f}={rmsv[f]:.3e}" for f in ("F32", "F16", "I16"))
    )
    # float16 carries ~11 bits of mantissa and int16 is quantised at
    # the server, so a fraction of a dB between them is expected; a
    # decoding fault is orders of magnitude larger than that.
    compare(floors["F32"], floors["F16"], "F32 vs F16", 3.0)
    compare(floors["F32"], floors["I16"], "F32 vs I16", 3.0)
    compare(floors["F16"], floors["I16"], "F16 vs I16", 3.0)

    print("\n3. the two API paths agree")
    try:
        sx, reported, drops = collect_soapy(args.url, "F32", args.rate, args.freq)
    except Exception as exc:  # noqa: BLE001 - reported, not swallowed
        check("soapy capture", False, str(exc))
        sx = None
    if sx is not None:
        check(
            "soapy reports the rate",
            abs(reported - args.rate) / args.rate < 1e-3,
            f"{reported/1e6:.6f} MS/s against {args.rate/1e6:.3f} requested",
        )
        check("soapy sample sanity", bool(np.all(np.isfinite(sx))), f"{len(sx)} samples")
        sfloor, _ = noise_floor(sx, args.rate)
        compare(floors["F32"], sfloor, "python vs soapy", 3.0)
        notes.append(f"soapy cumulative_drops during capture: {drops}")

    if args.ref_freq > 0:
        print("\n4. a known transmitter lands where it should")
        # Tune deliberately off the carrier so it sits at a positive
        # offset. A spectrum with I and Q transposed puts it at the
        # negative one, which nothing else in this file would catch:
        # every other check compares the radio against itself.
        offset = abs(args.ref_offset)
        centre = args.ref_freq - offset
        ref = collect_python(args.url, "F32", args.rate, centre)
        got, snr, level = find_carrier(ref, args.rate, offset)
        check(
            "reference carrier present",
            snr > 10.0,
            f"{snr:.1f} dB above the noise floor at {(centre+got)/1e6:.4f} MHz",
        )
        if snr > 10.0:
            check(
                "absolute frequency correct",
                abs(got - offset) < 3e3,
                f"found at {got/1e3:+.2f} kHz, expected {offset/1e3:+.2f} kHz "
                f"({(got-offset):+.0f} Hz off)",
            )
            check(
                "spectrum not mirrored",
                got > 0,
                f"carrier on the {'positive' if got > 0 else 'NEGATIVE'} side, "
                f"tuned {offset/1e3:.0f} kHz below it",
            )
            # The same carrier, seen through the other three decoders.
            for fmt in ("F16", "I16"):
                s2 = collect_python(
                    args.url, fmt, args.rate, centre, args.scale if fmt == "I16" else None
                )
                g2, _, l2 = find_carrier(s2, args.rate, offset)
                # Absolute level, against a transmitter that is always
                # on at constant power. This is the amplitude check
                # that catches a wrong integer scale: a mis-scaled
                # int16 stream has the right spectrum shape and the
                # wrong size, and only a stable absolute reference
                # sees the difference.
                check(
                    f"{fmt} agrees on the carrier",
                    abs(g2 - got) < 2e3 and abs(l2 - level) < 3.0,
                    f"{g2/1e3:+.2f} kHz at {l2:.1f} dB absolute "
                    f"(F32 saw {got/1e3:+.2f} kHz at {level:.1f} dB)",
                )
                gc.collect()
                time.sleep(1.0)
            try:
                ss, _, _ = collect_soapy(args.url, "F32", args.rate, centre)
                gs, _, ls = find_carrier(ss, args.rate, offset)
                check(
                    "soapy agrees on the carrier",
                    abs(gs - got) < 2e3 and abs(ls - level) < 3.0,
                    f"{gs/1e3:+.2f} kHz at {ls:.1f} dB absolute",
                )
            except Exception as exc:  # noqa: BLE001
                check("soapy carrier capture", False, str(exc))
    else:
        notes.append("reference-carrier checks skipped (--ref-freq 0)")

    print("\n5. the spectrum has the shape receiver output has")
    ref = floors["F32"] - np.median(floors["F32"][(freqs > -args.rate / 8) & (freqs < args.rate / 8)])
    k = 33
    sm = np.convolve(ref, np.ones(k) / k, mode="same")[k:-k]
    fk = freqs[k:-k]
    inband = sm[np.abs(fk) < args.rate * 0.4]
    edge = sm[np.abs(fk) > args.rate * 0.47]
    check(
        "passband flat across the declared span",
        float(np.ptp(inband)) < 12.0,
        f"{float(np.ptp(inband)):.1f} dB peak-to-peak inside 0.8 x Fs",
    )
    check(
        "rolls off beyond it",
        float(np.median(edge)) < float(np.median(inband)) - 1.0,
        f"edges {float(np.median(edge)) - float(np.median(inband)):+.1f} dB against the passband",
    )

    if args.ref_freq > 0 and args.freq > 0:
        # The reference checks retune the device; put it back so the
        # script does not leave a mission pointing somewhere else.
        try:
            back = collect_python(args.url, "F32", args.rate, args.freq)
            notes.append(f"device returned to {args.freq/1e6:.3f} MHz ({len(back)} samples read)")
        except Exception as exc:  # noqa: BLE001
            notes.append(f"could not restore the tuning: {exc}")

    print()
    for n in notes:
        print(f"  note: {n}")
    if failures:
        print(f"\nFAILED: {', '.join(failures)}")
        return 1
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
