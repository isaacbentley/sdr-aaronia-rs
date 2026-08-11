#!/usr/bin/env python3
"""Stream IQ samples from an RTSA-Suite HTTP server into PyArrow.

Reads come back as a FixedSizeListArray of [re, im] float32 pairs —
one copy out of the Rust receive buffer, then a copy-free Arrow C-Data
handoff into pyarrow. (Not "zero-copy": one copy per read is the honest
count.)

Usage:
    python python_arrow_example.py [http://host:54664]
"""

import sys

import aaronia

url = sys.argv[1] if len(sys.argv) > 1 else "http://localhost:54664"

config = aaronia.AaroniaConfig()
config.http_base_url = url          # pins the HTTP backend
config.center_freq = 2.44e9         # Hz
config.sample_rate = 15.36e6        # Hz

source = aaronia.AaroniaSource()
try:
    source.start_streaming(config)
except aaronia.AaroniaConnectionError as e:
    raise SystemExit(f"cannot reach {url}: {e}")

try:
    for i in range(5):
        # Blocks (GIL released) until 16384 samples arrive or the
        # internal 30 s timeout raises AaroniaTimeoutError.
        batch = source.read_samples_arrow(16384)
        first = batch[0].as_py()
        print(
            f"batch {i}: {len(batch)} IQ pairs, "
            f"first = {first[0]:+.3e} {first[1]:+.3e}j"
        )
finally:
    source.stop_streaming()
