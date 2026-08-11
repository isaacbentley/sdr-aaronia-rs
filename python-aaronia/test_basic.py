"""Offline sanity tests for the `aaronia` Python module.

These run without hardware: config field round-trips, validation
errors, and not-streaming error typing. Live-device coverage lives in
the repo's live test scripts (requires an RTSA HTTP server).
"""

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
