#!/usr/bin/env python3
"""Builds vector.ico (Windows) and vector.icns (macOS) from the PNGs in packaging/icons, without external tools.
The PNGs come from the app itself:  QT_QPA_PLATFORM=offscreen ./build/vector --write-icons packaging/icons"""
import os, struct

here = os.path.dirname(os.path.abspath(__file__))
icons = os.path.join(here, "icons")

def png(size):
    with open(os.path.join(icons, f"icon-{size}.png"), "rb") as f:
        return f.read()

# ICO: a directory of PNG images (supported since Windows Vista)
sizes = [16, 32, 48, 64, 128, 256]
data = [png(s) for s in sizes]
out = struct.pack("<HHH", 0, 1, len(sizes))
offset = 6 + 16 * len(sizes)
for s, d in zip(sizes, data):
    out += struct.pack("<BBBBHHII", 0 if s == 256 else s, 0 if s == 256 else s, 0, 0, 1, 32, len(d), offset)
    offset += len(d)
open(os.path.join(here, "vector.ico"), "wb").write(out + b"".join(data))

# ICNS: PNG payloads (ic07 128, ic08 256, ic09 512, icp4 16, icp5 32, icp6 64)
chunks = b""
for tag, s in [(b"icp4", 16), (b"icp5", 32), (b"icp6", 64), (b"ic07", 128), (b"ic08", 256), (b"ic09", 512)]:
    d = png(s)
    chunks += tag + struct.pack(">I", len(d) + 8) + d
open(os.path.join(here, "vector.icns"), "wb").write(b"icns" + struct.pack(">I", len(chunks) + 8) + chunks)
print("wrote vector.ico and vector.icns")
