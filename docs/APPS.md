# Using a SPECTRAN from existing SDR applications

Any SoapySDR-capable application can drive a SPECTRAN through the
[`soapy-aaronia`](../soapy-aaronia/README.md) plugin. Install the plugin
first. Prebuilt binaries are attached to each release. Confirm it is
visible:

```bash
SoapySDRUtil --check=aaronia
```

Every application below uses the same device string. In most cases the
RTSA-Suite HTTP server URL is the only argument needed:

```
driver=aaronia,url=http://localhost:54664
```

Add `format=I16` when streaming across a network to halve the wire
bandwidth. The full argument list is in the
[plugin README](../soapy-aaronia/README.md#device-arguments).

All of these depend on a configured RTSA-Suite mission. See
[QUICKSTART.md](QUICKSTART.md) if you have not set one up.

## SDR++

Add a SoapySDR source and select the Aaronia device from the source
dropdown. If it does not appear, launch SDR++ from a terminal with
`SOAPY_SDR_PLUGIN_PATH` set to the directory containing the module.
SDR++ enumerates plugins at startup, so the variable must be set
beforehand.

Enumeration advertises a `localhost:54664` candidate without probing it,
because `find()` must not block on the network. For a remote server, use
SDR++'s manual device string field with your own `url=`.

## GQRX

GQRX accepts a device string directly in its configuration dialog:

```
driver=aaronia,url=http://localhost:54664
```

Select the sample rate from GQRX's list. The plugin advertises 250 kHz,
500 kHz, 1, 2, 5, 10, 15.36, 20, 30.72 and 61.44 MHz for applications
that build dropdowns from `listSampleRates`. Rates within the reported
range also work over HTTP, so a rate absent from the list is not
necessarily unsupported.

The single gain element is `REF`, the Aaronia reference level in dBm. It
is not an amplifier gain: raising it reduces sensitivity. Start near
−20 dBm and lower it if the noise floor is too high.

## GNU Radio

Use the **Soapy Custom Source** block from `gr-soapy`:

- Device string: `driver=aaronia,url=http://localhost:54664`
- Sample rate: a rate from the list above
- Center frequency: as required. Retuning while running is safe.
- Gain (`REF`): reference level in dBm

The block outputs `complex64`, matching the plugin's native `CF32`
format.

## SoapySDR from Python

```python
import SoapySDR
from SoapySDR import SOAPY_SDR_RX, SOAPY_SDR_CF32
import numpy as np

sdr = SoapySDR.Device("driver=aaronia,url=http://localhost:54664")
sdr.setSampleRate(SOAPY_SDR_RX, 0, 15.36e6)
sdr.setFrequency(SOAPY_SDR_RX, 0, 2.44e9)

rx = sdr.setupStream(SOAPY_SDR_RX, SOAPY_SDR_CF32)
sdr.activateStream(rx)

buff = np.empty(65536, np.complex64)
status = sdr.readStream(rx, [buff], len(buff), timeoutUs=int(1e6))
print(status.ret, buff[:4])

sdr.deactivateStream(rx)
sdr.closeStream(rx)
```

A complete version is in
[`examples/soapy_python_example.py`](../examples/soapy_python_example.py).

For NumPy and Arrow work, the
[`python-aaronia`](../python-aaronia/README.md) bindings call the crate
directly and skip the SoapySDR layer.

## General notes

- Sample rate is the Aaronia "span", not the usable RF bandwidth. The
  alias-free span is smaller, roughly 49 MHz within a 61.44 MHz capture.
- `readStream` honours `timeoutUs` and returns partial reads within the
  deadline, per the SoapySDR contract.
- Retuning mid-stream is safe and requires no Aaronia licence.
- Dropped blocks are reported through `readSensor("cumulative_drops")`.
  An overrun sets `SOAPY_SDR_END_ABRUPT` on the affected read.
- TX is hardware-unverified and exists only when the plugin is built
  against the native SDK on Windows or Linux.
