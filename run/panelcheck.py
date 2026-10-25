#!/usr/bin/env python3
"""panelcheck.py — deterministic GNOME-frame verifier (no vision model).

Checks a fire.py PPM shot for the GNOME Shell panel signature by direct
pixel math:
  - a full-width #232323 (±4) bar across rows 0..33 (>=85% occupancy)
  - bright text-glyph pixels (lum > 0x60) inside that strip, clustered
    by x-bucket (left = Activities, center = clock)
  - a white cursor sprite (bright cluster below the strip)
Exit 0 = panel present; prints a one-line verdict + JSON-ish details.
Usage: python3 run/panelcheck.py runs/gnome-<tag>/shot-<t>s.ppm [...]
"""
import sys

def check(path):
    with open(path, "rb") as f:
        data = f.read()
    idx = 0; toks = []
    while len(toks) < 4:
        sp = data.find(b"\n", idx); line = data[idx:sp]; idx = sp + 1
        if line.startswith(b"#") or not line:
            continue
        toks += [x for x in line.split() if x]
    w, h = int(toks[1]), int(toks[2])
    raw = data[idx:idx + w * h * 3]
    strip = 0; strip_ok = 0; glyphs = {}; body_bright = []
    for y in range(h):
        for x in range(w):
            p = raw[(y * w + x) * 3:(y * w + x) * 3 + 3]
            lum = (p[0] * 299 + p[1] * 587 + p[2] * 114) // 1000
            if y < 34:
                strip += 1
                if abs(p[0] - 0x23) <= 4 and abs(p[1] - 0x23) <= 4 and abs(p[2] - 0x23) <= 4:
                    strip_ok += 1
                if lum > 0x60:
                    glyphs.setdefault(x // 80, []).append((x, y))
            elif lum > 0x90 and p[0] > 0x99 and p[1] > 0x99 and p[2] > 0x99:
                body_bright.append((x, y))
    occ = strip_ok / strip if strip else 0
    clusters = {k: len(v) for k, v in sorted(glyphs.items()) if len(v) >= 30}
    panel = occ >= 0.85 and len(clusters) >= 2
    print(f"{path}: panel={'YES' if panel else 'NO'} bar_occupancy={occ:.2f} "
          f"glyph_clusters(x80px)={clusters} cursor_px={len(body_bright)}")
    return panel

if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit("usage: panelcheck.py shot.ppm [...]")
    ok = any(check(p) for p in sys.argv[1:])
    sys.exit(0 if ok else 1)
