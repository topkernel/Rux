#!/usr/bin/env python3
# r41-ppm2png.py — pure-python PPM(P6) -> PNG converter (no external tools).
# Used by the pixel hunt: a shot whose md5 leaves the frozen set gets
# converted to PNG for the report. Usage: r41-ppm2png.py in.ppm out.png
import struct, sys, zlib

def ppm_to_png(src, dst):
    with open(src, "rb") as f:
        data = f.read()
    if data[:2] != b"P6":
        raise SystemExit(f"not a P6 ppm: {src}")
    pos = 2
    fields = []
    while len(fields) < 3:
        # skip whitespace/comments
        while pos < len(data) and data[pos : pos + 1].isspace():
            pos += 1
        if data[pos : pos + 1] == b"#":
            while pos < len(data) and data[pos : pos + 1] != b"\n":
                pos += 1
            continue
        start = pos
        while pos < len(data) and not data[pos : pos + 1].isspace():
            pos += 1
        fields.append(int(data[start:pos]))
    pos += 1  # single whitespace after maxval
    w, h, maxval = fields
    raw = data[pos : pos + w * h * 3]
    assert maxval == 255 and len(raw) == w * h * 3, (maxval, len(raw), w * h * 3)

    def chunk(tag, payload):
        c = struct.pack(">I", len(payload)) + tag + payload
        return c + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF)

    scan = b"".join(b"\x00" + raw[y * w * 3 : (y + 1) * w * 3] for y in range(h))
    png = (b"\x89PNG\r\n\x1a\n"
           + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
           + chunk(b"IDAT", zlib.compress(scan, 6))
           + chunk(b"IEND", b""))
    with open(dst, "wb") as f:
        f.write(png)
    return w, h, len(png)

if __name__ == "__main__":
    w, h, n = ppm_to_png(sys.argv[1], sys.argv[2])
    print(f"{sys.argv[2]}: {w}x{h} {n}B")
