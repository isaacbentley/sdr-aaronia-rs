"""Type stubs for the `aaronia` extension module.

Kept in sync by hand with `src/lib.rs`; maturin packages this file and
the accompanying `py.typed` marker into the wheel.
"""

from typing import Any, Iterator, List, Literal, Optional, Tuple

import numpy as np
import numpy.typing as npt

__all__ = [
    "SpectranConfig",
    "SpectranSource",
    "BlockIterator",
    "SpectranConnectionError",
    "SpectranHardwareError",
    "SpectranTimeoutError",
    "SpectranStreamClosed",
    "open",
    "sample_rates",
    "sample_rate_for_bandwidth",
    "diagnose",
]

class SpectranConnectionError(Exception):
    """The RTSA HTTP endpoint or device could not be reached."""

class SpectranHardwareError(Exception):
    """The device or SDK reported an error."""

class SpectranTimeoutError(Exception):
    """A read or control operation timed out."""

class SpectranStreamClosed(SpectranConnectionError):
    """The sample stream ended and will produce no more data.

    Subclasses :class:`SpectranConnectionError`, so existing handlers
    still catch it. :meth:`SpectranSource.blocks` ends the iteration on
    this and only this.
    """

WireFormat = Literal["F32", "F16", "I16"]
ReceiverChannel = Literal["Rx1", "Rx2", "Rx1And2"]

class SpectranConfig:
    """Configuration for a :class:`SpectranSource`.

    Setting ``http_base_url`` selects the HTTP backend; setting
    ``file_path`` selects file playback. Every property is readable and
    writable.
    """

    def __init__(self) -> None: ...

    # Backend selection.
    http_base_url: Optional[str]
    file_path: Optional[str]
    device_serial: Optional[str]
    force_native_sdk: bool

    # RF parameters.
    center_frequency_hz: float
    """Center frequency in Hz."""
    sample_rate_hz: float
    """IQ sample rate (Fs) in Hz. The device key behind it is `spanfreq`."""
    reference_level_dbm: float
    """Reference level in dBm."""
    scale: Optional[float]
    """Integer encode multiplier for the ``I16`` wire format
    (``/stream?scale=N``), or None for the server default.

    The server encodes ``round(value * scale)``, so the quantisation
    step is ``1 / scale``. At the default the step is 1/16384, which is
    coarser than a quiet band's noise floor — measured on a live
    server, 70% of samples came back exactly zero. Raise it, or raise
    the gain by lowering ``reference_level_dbm``. Must be positive and
    finite."""

    # Transport behaviour. `format` and `receiver_channel` read back as
    # None when unset but only accept a string: assigning None raises
    # TypeError, so getter and setter are declared separately.
    @property
    def format(self) -> Optional[WireFormat]:
        """HTTP wire format, or None when unset."""

    @format.setter
    def format(self, value: WireFormat) -> None:
        """Assigning an unrecognised string raises ``ValueError``."""

    @property
    def receiver_channel(self) -> Optional[ReceiverChannel]:
        """Native-SDK receiver channel, or None when unset."""

    @receiver_channel.setter
    def receiver_channel(self, value: ReceiverChannel) -> None:
        """Assigning an unrecognised string raises ``ValueError``."""

    read_timeout_s: float
    """Seconds a blocking read waits before ``SpectranTimeoutError``
    (default 30.0). Must be positive and finite."""
    auto_reconnect: bool
    """Reconnect the HTTP stream after a drop (default ``True``)."""

class SpectranSource:
    """A streaming IQ source.

    Construct, call :meth:`start_streaming`, then read. Blocking calls
    release the GIL, so other Python threads continue to run and
    ``KeyboardInterrupt`` is delivered between calls.

    Every method other than :meth:`start_streaming` and
    :meth:`stop_streaming` raises ``SpectranHardwareError`` when the
    source is not streaming.
    """

    def __init__(self) -> None: ...
    def __enter__(self) -> "SpectranSource":
        """Return the already-streaming source."""

    def __exit__(self, exc_type: Any, exc_value: Any, traceback: Any) -> bool:
        """Stop streaming. Never suppresses an exception."""

    def blocks(self, count: int) -> "BlockIterator":
        """Iterate ``count``-sample blocks until the stream closes.

        Ends on ``SpectranStreamClosed`` or an empty read (a recording
        running out). Timeouts and connection failures still raise, so
        a truncated capture is not mistaken for a finished one.
        """

    def start_streaming(self, config: SpectranConfig) -> None:
        """Connect to the backend selected by ``config`` and start streaming.

        Raises ``SpectranConnectionError`` if the endpoint is
        unreachable, ``ValueError`` for invalid configuration, and
        ``SpectranHardwareError`` for device or SDK failures.
        """

    def stop_streaming(self) -> None:
        """Stop streaming and release the backend."""

    def read_samples_numpy(self, count: int) -> npt.NDArray[np.complex64]:
        """Read up to ``count`` IQ samples into a NumPy ``complex64`` array.

        A read never spans a retune: if the centre frequency changes
        part-way through, the read returns the samples captured before
        it and the rest arrive on the next call. Expect a short array
        around a :meth:`set_center_frequency_hz`, rather than one whose
        samples come from two different frequencies.

        Raises ``SpectranTimeoutError`` if no data arrives within
        ``config.read_timeout_s``, ``SpectranConnectionError`` if the
        stream is closed, and ``ValueError`` if ``count`` exceeds the
        per-read limit of 2**26 samples.
        """

    def read_samples_arrow(self, count: int) -> Any:
        """Read up to ``count`` IQ samples as a PyArrow
        ``FixedSizeListArray`` of ``[re, im]`` float32 pairs.

        Raises the same exceptions as :meth:`read_samples_numpy`.
        """

    def read_samples_dual_numpy(
        self, count: int
    ) -> Tuple[npt.NDArray[np.complex64], npt.NDArray[np.complex64]]:
        """Read ``count`` time-aligned ``(rx1, rx2)`` sample pairs.

        Requires ``receiver_channel = "Rx1And2"`` on the native-SDK
        backend. This path is hardware-unverified. Raises the same
        exceptions as :meth:`read_samples_numpy`.
        """

    def set_center_frequency_hz(self, freq_hz: float) -> None:
        """Retune the running source without tearing it down."""

    def set_sample_rate_hz(self, rate_hz: float) -> None: ...
    def set_reference_level_dbm(self, dbm: float) -> None: ...
    def take_overrun(self) -> bool:
        """True once per detected receive-side overrun, then cleared."""

    def cumulative_drops(self) -> int:
        """Number of timestamp gaps detected in the stream so far (gap events, not samples)."""

    def last_timestamp_ns(self) -> int:
        """Epoch-nanosecond timestamp of the most recent block.
        HTTP backend only; 0 otherwise."""

    def master_stream_time_ns(self) -> Optional[int]:
        """The device's master stream clock, in epoch nanoseconds.

        The timebase the device paces streams against, readable before
        the first block arrives. Native-SDK backend only; None otherwise."""

    def gps_time_ns(self) -> Optional[int]:
        """The latest GPS time in epoch nanoseconds, or None without a fix.

        Native-SDK backend only, and only once ``device/gpsmode`` is
        enabled — the device ships with GPS off."""

class BlockIterator:
    """Iterator returned by :meth:`SpectranSource.blocks`."""

    def __iter__(self) -> "BlockIterator": ...
    def __next__(self) -> npt.NDArray[np.complex64]: ...

def open(
    url: Optional[str] = None,
    *,
    center_frequency_hz: Optional[float] = None,
    sample_rate_hz: Optional[float] = None,
    bandwidth_hz: Optional[float] = None,
    reference_level_dbm: Optional[float] = None,
    file: Optional[str] = None,
    sdk: bool = False,
    serial: Optional[str] = None,
    format: Optional[WireFormat] = None,
    scale: Optional[float] = None,
    read_timeout_s: Optional[float] = None,
) -> SpectranSource:
    """Open a source and start streaming, in one call.

    Give either ``sample_rate_hz`` (an exact rate) or ``bandwidth_hz`` (how
    much spectrum to cover, from which a real rate is chosen); passing
    both raises ``ValueError``, as does passing both ``url`` and
    ``file``. With neither ``url`` nor ``file``, connects to
    ``http://localhost:54664``.
    """

def sample_rates() -> List[float]:
    """The IQ sample rates the hardware can run, highest first."""

def sample_rate_for_bandwidth(bandwidth_hz: float) -> float:
    """The lowest sample rate whose alias-free bandwidth covers
    ``bandwidth_hz``."""

def diagnose(url: str = "http://localhost:54664") -> List[Tuple[bool, str, str]]:
    """Check an RTSA-Suite HTTP server.

    Returns ``(ok, message, fix)`` for each check. The
    ``aaronia-doctor`` console script prints the same results.
    """

def doctor_cli() -> int:
    """Entry point behind the ``aaronia-doctor`` console script.

    Runs :func:`diagnose` against the default server, prints the results,
    and returns a process exit code — ``0`` when every check passed.
    Exported from the module, so it is declared here; call
    :func:`diagnose` instead if you want the results as data.
    """
