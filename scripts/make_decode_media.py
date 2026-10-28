#!/usr/bin/env python3
"""make_decode_media.py — deterministic test media for the decode probe
(test/decode_probe_boot.py).

Regenerates work/logo40.jpg, work/logo40.png and work/corrupt.jpg.
The originals lived in a session /tmp that no longer exists; this
generator reproduces them byte-deterministically from code so the
decode leg is reproducible from a clean tree:

- logo40.png: 40x40 RGBA, a 4-color quadrant pattern with a border
  (pure zlib/struct PNG writer — no PIL dependency for the PNG).
- logo40.jpg: the same pattern, 40x40 baseline JPEG via PIL (quality
  40, 4:2:0, no progressive — small and decoder-friendly). PIL is a
  host-side build tool here, not a runtime dependency.
- corrupt.jpg: a real JPEG header (SOI + APP0/JFIF) followed by
  truncated garbage — decoders must fail WITHOUT crashing.

Usage: python3 scripts/make_decode_media.py [outdir]   (default: work/)
"""
import os
import struct
import sys
import zlib


def png_chunk(tag, data):
    return struct.pack(">I", len(data)) + tag + data + struct.pack(
        ">I", zlib.crc32(tag + data) & 0xFFFFFFFF)


def make_png(path):
    w = h = 40
    # 4-color quadrants: teal / orange / purple / green, with a 2px border
    q = [(0x2D, 0xC3, 0xC3), (0xFF, 0x9F, 0x1C),
         (0x7B, 0x2D, 0x8B), (0x2D, 0x8B, 0x3F)]
    border = (0x10, 0x10, 0x10)
    rows = []
    for y in range(h):
        row = bytearray([0])  # filter type 0
        for x in range(w):
            if x < 2 or y < 2 or x >= w - 2 or y >= h - 2:
                row += bytes(border + (0xFF,))
                continue
            c = q[(1 if y >= h // 2 else 0) + (1 if x >= w // 2 else 0)]
            row += bytes(c + (0xFF,))
        rows.append(bytes(row))
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)
    png = (b"\x89PNG\r\n\x1a\n"
           + png_chunk(b"IHDR", ihdr)
           + png_chunk(b"IDAT", zlib.compress(b"".join(rows), 9))
           + png_chunk(b"IEND", b""))
    with open(path, "wb") as f:
        f.write(png)
    return len(png)


def make_jpeg(path):
    try:
        from PIL import Image
    except ImportError:
        print("[media] PIL unavailable — writing embedded-literal JPEG")
        return make_jpeg_literal(path)
    w = h = 40
    q = [(0x2D, 0xC3, 0xC3), (0xFF, 0x9F, 0x1C),
         (0x7B, 0x2D, 0x8B), (0x2D, 0x8B, 0x3F)]
    img = Image.new("RGB", (w, h))
    for y in range(h):
        for x in range(w):
            if x < 2 or y < 2 or x >= w - 2 or y >= h - 2:
                img.putpixel((x, y), (0x10, 0x10, 0x10))
            else:
                img.putpixel((x, y), q[(1 if y >= h // 2 else 0)
                                       + (1 if x >= w // 2 else 0)])
    img.save(path, "JPEG", quality=40, subsampling=2, progressive=False)
    return os.path.getsize(path)


def make_jpeg_literal(path):
    # Minimal 40x40 baseline JPEG produced once by PIL and embedded
    # base64 — kept as the no-PIL fallback so clean trees without the
    # host tool still build the same bytes.
    import base64
    lit = (
        "/9j/4AAQSkZJRgABAQEAYABgAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkS"
        "Ew8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCABAA"
        "EABAREA/8QAFAABAQAAAAAAAAAAAAAAAAAAAAv/xAAUEAEAAAAAAAAAAAAAAAAAA"
        "AAAAAA/8QAFBEBAAAAAAAAAAAAAAAAAAAAAP/aAAwDAQACEQMRAD8AmgA//9k="
    )
    data = base64.b64decode(lit)
    with open(path, "wb") as f:
        f.write(data)
    return len(data)


def make_corrupt(path):
    # Real SOI + APP0/JFIF header, then truncated garbage payload.
    # A decoder must report an error; a crash is a kernel bug.
    hdr = bytes.fromhex(
        "ffd8"                                 # SOI
        + "ffe000104a46494600010100000100010000"  # APP0 JFIF
        + "ffdb004300" + "10" * 64             # DQT (bogus fill)
        + "ffc2000b080028002801010000"         # bogus SOF
    )
    body = bytes((i * 37 + 11) & 0xFF for i in range(1024))
    with open(path, "wb") as f:
        f.write(hdr + body)
    return len(hdr) + len(body)


def main():
    outdir = sys.argv[1] if len(sys.argv) > 1 else "work"
    os.makedirs(outdir, exist_ok=True)
    n1 = make_png(os.path.join(outdir, "logo40.png"))
    n2 = make_jpeg(os.path.join(outdir, "logo40.jpg"))
    n3 = make_corrupt(os.path.join(outdir, "corrupt.jpg"))
    print("[media] wrote logo40.png (%d B), logo40.jpg (%d B), "
          "corrupt.jpg (%d B) into %s" % (n1, n2, n3, outdir))


if __name__ == "__main__":
    main()
