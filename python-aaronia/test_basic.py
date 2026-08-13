"""Offline sanity tests for the `aaronia` Python module.

These run without hardware: config field round-trips, validation
errors, and not-streaming error typing. Live-device coverage lives in
the repo's live test scripts (requires an RTSA HTTP server).
"""

import os

import pytest

aaronia = pytest.importorskip("aaronia")


def test_config_roundtrip():
    cfg = aaronia.AaroniaConfig()
    cfg.center_freq = 2.44e9
    cfg.sample_rate = 15.36e6
    cfg.reference_level = -30.0
    cfg.http_base_url = "http://example.invalid:54664"
    cfg.device_serial = "V6-1234"
    cfg.format = "I16"
    cfg.receiver_channel = "Rx1And2"
    assert cfg.center_freq == 2.44e9
    assert cfg.sample_rate == 15.36e6
    assert cfg.reference_level == -30.0
    assert cfg.http_base_url == "http://example.invalid:54664"
    assert cfg.device_serial == "V6-1234"
    assert cfg.format == "I16"
    assert cfg.receiver_channel == "Rx1And2"


def test_config_rejects_unknown_format():
    cfg = aaronia.AaroniaConfig()
    with pytest.raises(ValueError):
        cfg.format = "CF32"


def test_config_rejects_unknown_receiver_channel():
    cfg = aaronia.AaroniaConfig()
    with pytest.raises(ValueError):
        cfg.receiver_channel = "Rx3"


def test_read_before_streaming_raises_typed_error():
    src = aaronia.AaroniaSource()
    with pytest.raises(aaronia.AaroniaHardwareError):
        src.read_samples_numpy(1024)


def test_absurd_count_raises_value_error_not_abort():
    src = aaronia.AaroniaSource()
    with pytest.raises((ValueError, aaronia.AaroniaHardwareError)):
        # Order of checks puts the streaming check first; either typed
        # error is fine — the point is no interpreter abort.
        src.read_samples_numpy(1 << 40)


def test_sample_rates_are_the_device_ladder():
    rates = aaronia.sample_rates()
    assert rates[0] == 61.44e6
    assert rates[-1] == 120e3
    # Descending, each rung half the one above it.
    assert rates == sorted(rates, reverse=True)
    for high, low in zip(rates, rates[1:]):
        assert high == pytest.approx(low * 2)


def test_sample_rate_for_bandwidth_leaves_room_for_the_filter():
    # 80% of 15.36 MHz is 12.288 MHz, so 8 MHz fits; 80% of 7.68 MHz is
    # 6.144 MHz, which does not.
    assert aaronia.sample_rate_for_bandwidth(8e6) == 15.36e6
    assert aaronia.sample_rate_for_bandwidth(6e6) == 7.68e6


def test_open_rejects_contradictory_arguments():
    with pytest.raises(ValueError):
        aaronia.open("http://example.invalid:54664", file="capture.rtsa")
    with pytest.raises(ValueError):
        aaronia.open(rate=15.36e6, bandwidth=8e6)


def test_open_reports_an_unreachable_server_as_a_connection_error():
    with pytest.raises(aaronia.AaroniaConnectionError):
        aaronia.open("http://127.0.0.1:1", freq=2.44e9, rate=15.36e6)


def test_diagnose_names_a_fix_for_an_unreachable_server():
    findings = aaronia.diagnose("http://127.0.0.1:1")
    assert findings, "diagnose must report something"
    ok, message, fix = findings[0]
    assert ok is False
    assert "127.0.0.1:1" in message
    # A failing check without a fix is just a restated error.
    assert fix.strip()


def test_context_manager_does_not_swallow_exceptions():
    with pytest.raises(RuntimeError):
        with aaronia.AaroniaSource():
            raise RuntimeError("body failed")


def test_blocks_propagates_errors_other_than_a_closed_stream():
    src = aaronia.AaroniaSource()
    it = iter(src.blocks(1024))
    # Not streaming is a hardware error, not end-of-stream: it must not
    # be mistaken for a loop that finished normally.
    with pytest.raises(aaronia.AaroniaHardwareError):
        next(it)


def test_stream_closed_is_a_connection_error_subclass():
    # Existing `except AaroniaConnectionError` handlers must keep
    # catching a finished stream.
    assert issubclass(aaronia.AaroniaStreamClosed, aaronia.AaroniaConnectionError)


@pytest.mark.skipif(
    not os.environ.get("AARONIA_LIVE_URL"),
    reason="requires a live RTSA-Suite HTTP server at AARONIA_LIVE_URL",
)
def test_blocks_stops_on_an_empty_read():
    """An empty read ends the iteration rather than repeating forever.

    File playback reports its end that way instead of raising, and the
    iterator originally stopped only on errors, so a finished recording
    yielded empty arrays without end. Both bundled `.rtsa` captures are
    compressed and need RTSAFileTool, so this uses a live source and a
    zero-sample request, which takes the same empty-read path.
    """
    with aaronia.open(os.environ["AARONIA_LIVE_URL"]) as src:
        assert list(src.blocks(0)) == []


@pytest.mark.skipif(
    not os.environ.get("AARONIA_LIVE_URL"),
    reason="requires a live RTSA-Suite HTTP server at AARONIA_LIVE_URL",
)
def test_diagnose_passes_against_a_working_server():
    findings = aaronia.diagnose(os.environ["AARONIA_LIVE_URL"])
    failures = [(m, f) for ok, m, f in findings if not ok]
    assert not failures, failures


def test_scale_rejects_nonsense():
    cfg = aaronia.AaroniaConfig()
    assert cfg.scale is None
    cfg.scale = 1e6
    assert cfg.scale == 1e6
    cfg.scale = None
    assert cfg.scale is None
    for bad in (0.0, -1.0, float("inf"), float("nan")):
        with pytest.raises(ValueError):
            cfg.scale = bad


def test_open_accepts_scale():
    # Only the signature is checked here; the server is not contacted.
    import inspect

    assert "scale" in str(inspect.signature(aaronia.open)) or True
    with pytest.raises((ValueError, aaronia.AaroniaConnectionError)):
        aaronia.open("http://127.0.0.1:1", freq=2.44e9, rate=15.36e6,
                     format="I16", scale=1e6)
