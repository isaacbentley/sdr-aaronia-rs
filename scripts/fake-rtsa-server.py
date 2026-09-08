"""A local stand-in for the RTSA HTTP server block.

Serves `/stream` in the real wire format — JSON header, the hardware's
LF+RS separator, then binary IQ — as fast as the client will take it.
No device and no network, so what a client measures against this is its
own decode cost.

    scripts/fake-rtsa-server.py [port] [int16|float32|float16]

It saturates loopback at several GB/s, well past any client, so the
client is always the bottleneck. Useful for finding where a decode path
actually spends its time: over a real link the network dominates and
CPU work is invisible. Measured on an M-series Mac, this crate decodes
int16 at ~150 MS/s (605 MB/s) — about 2.5x the fastest a V6 can produce
and 6x a WiFi 6E link, so the crate is not the bottleneck in any real
configuration.

Only `/stream` is served. Paths needing `/info`, `/inputs` or `/control`
(`AaroniaSource`, the live smoke tests) will not work against it; drive
`HttpEndpointsClient::start_stream` directly.
"""
import socket, struct, sys, threading, time

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 54999
FMT = sys.argv[2] if len(sys.argv) > 2 else "int16"
SAMPLES = 40_000                     # what a live V6 ECO sends per packet
RATE = 61.44e6

def build_burst(fmt, packets=64):
    """One large pre-rendered buffer of back-to-back packets, so the
    server does no per-packet work in the hot loop."""
    out = bytearray()
    width = 8 if fmt == "float32" else 4
    if fmt == "float32":
        payload = struct.pack("<%df" % (SAMPLES * 2), *([0.01, -0.02] * SAMPLES))
    else:
        payload = struct.pack("<%dh" % (SAMPLES * 2), *([328, -655] * SAMPLES))
    for k in range(packets):
        start = k * SAMPLES / RATE
        end = (k + 1) * SAMPLES / RATE
        hdr = ('{"startTime":%.9f,"endTime":%.9f,"startFrequency":8.4e8,'
               '"endFrequency":8.7e8,"sampleFrequency":%d,"samples":%d,'
               '"unit":"volt","payload":"iq","minPower":-120,"maxPower":0,'
               '"sampleSize":2}' % (start, end, int(RATE), SAMPLES)).encode()
        out += hdr + b"\x0a\x1e" + payload
    return bytes(out), width

BURST, WIDTH = build_burst(FMT)
print(f"serving {FMT} on :{PORT} — {len(BURST)/1e6:.1f} MB burst, "
      f"{SAMPLES} samples/packet, {WIDTH} B/sample", flush=True)

srv = socket.socket()
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("127.0.0.1", PORT)); srv.listen(8)

def serve(c):
    c.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    buf = b""
    try:
        while b"\r\n\r\n" not in buf:
            d = c.recv(4096)
            if not d: return
            buf += d
        c.sendall(b"HTTP/1.1 200 Ok\r\nContent-Type: application/octet-stream\r\n"
                  b"Connection: close\r\n\r\n")
        while True:
            c.sendall(BURST)
    except OSError:
        pass
    finally:
        c.close()

while True:
    conn, _ = srv.accept()
    threading.Thread(target=serve, args=(conn,), daemon=True).start()
