"""Type stubs for the `aaronia` extension module.

Kept in sync by hand with `src/lib.rs`; maturin packages this file and
the accompanying `py.typed` marker into the wheel.
"""

from typing import Any, Literal, Optional, Tuple

import numpy as np
import numpy.typing as npt

__all__ = [
    "AaroniaConfig",
    "AaroniaSource",
    "AaroniaConnectionError",
    "AaroniaHardwareError",
    "AaroniaTimeoutError",
]

class AaroniaConnectionError(Exception):
    """The RTSA HTTP endpoint or device could not be reached."""

class AaroniaHardwareError(Exception):
    """The device or SDK reported an error."""

class AaroniaTimeoutError(Exception):
    """A read or control operation timed out."""

WireFormat = Literal["F32", "F16", "I16"]
ReceiverChannel = Literal["Rx1", "Rx2", "Rx1And2"]

class AaroniaConfig:
    """Configuration for an :class:`AaroniaSource`.

    Setting ``http_base_url`` selects the HTTP backend; setting
    ``file_path`` selects file playback. Every property is readable and
    writable.
    """

    def __init__(self) -> None: ...

    # Backend selection.
    http_base_url: Optional[str]
    file_path: Optional[str]
    device_serial: Optional[str]

    # RF parameters.
    center_freq: float
    """Center frequency in Hz."""
    sample_rate: float
    """IQ sample rate in Hz (the Aaronia "span")."""
    reference_level: float
    """Reference level in dBm."""

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

    read_timeout: float
    """Seconds a blocking read waits before ``AaroniaTimeoutError``
    (default 30.0). Must be positive and finite."""
    auto_reconnect: bool
    """Reconnect the HTTP stream after a drop (default ``True``)."""

class AaroniaSource:
    """A streaming IQ source.

    Construct, call :meth:`start_streaming`, then read. Blocking calls
    release the GIL, so other Python threads continue to run and
    ``KeyboardInterrupt`` is delivered between calls.

    Every method other than :meth:`start_streaming` and
    :meth:`stop_streaming` raises ``AaroniaHardwareError`` when the
    source is not streaming.
    """

    def __init__(self) -> None: ...
    def start_streaming(self, config: AaroniaConfig) -> None:
        """Connect to the backend selected by ``config`` and start streaming.

        Raises ``AaroniaConnectionError`` if the endpoint is
        unreachable, ``ValueError`` for invalid configuration, and
        ``AaroniaHardwareError`` for device or SDK failures.
        """

    def stop_streaming(self) -> None:
        """Stop streaming and release the backend."""

    def read_samples_numpy(self, count: int) -> npt.NDArray[np.complex64]:
        """Read up to ``count`` IQ samples into a NumPy ``complex64`` array.

        Raises ``AaroniaTimeoutError`` if no data arrives within
        ``config.read_timeout``, ``AaroniaConnectionError`` if the
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

    def set_center_frequency(self, freq_hz: float) -> None:
        """Retune the running source without tearing it down."""

    def set_sample_rate(self, rate_hz: float) -> None: ...
    def set_reference_level(self, dbm: float) -> None: ...
    def take_overrun(self) -> bool:
        """True once per detected receive-side overrun, then cleared."""

    def cumulative_drops(self) -> int:
        """Total samples the drop detector has attributed to gaps."""

    def last_timestamp_ns(self) -> int:
        """Epoch-nanosecond timestamp of the most recent block.
        HTTP backend only; 0 otherwise."""
