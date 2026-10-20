"""Converts the raw screenshot written by the app's CKARAOKE_SCREENSHOT test aid to a PNG.

    png_from_raw.py <raw-file> <out.png>
The raw file starts with a line "<width> <height>", followed by RGBA bytes, row by row.
"""
import struct
import sys
import zlib


def png(width, height, rgba):
    rows = b"".join(b"\x00" + rgba[y * width * 4:(y + 1) * width * 4] for y in range(height))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(rows, 6)) + chunk(b"IEND", b"")


raw = open(sys.argv[1], "rb").read()
line_end = raw.index(b"\n")
width, height = map(int, raw[:line_end].split())
open(sys.argv[2], "wb").write(png(width, height, raw[line_end + 1:]))
print(f"wrote {sys.argv[2]} ({width}x{height})")
