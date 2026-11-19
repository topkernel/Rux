#!/usr/bin/env python3
"""shotstat.py — pixel statistics for an OH boot screendump (PPM P6).

Usage: python3 run/shotstat.py <shot.ppm>

Reports: dimensions, distinct colors, non-black share, and a coarse
row/column profile so 'static logo' vs 'rendered UI' is obvious from
the numbers (a logo is one small gray cluster; a rendered OH UI has
thousands of colors spread over the panel).
"""
import sys
from collections import Counter

def main(path):
    data = open(path, 'rb').read()
    if data[:2] != b'P6':
        print('not a P6 ppm:', path); return 1
    # header: P6\n<w> <h>\n<max>\n
    parts = data.split(b'\n', 3)
    w, h = map(int, parts[1].split())
    px = parts[3]
    n = w * h
    colors = Counter()
    nonblack = 0
    for i in range(0, min(len(px), n * 3) - 2, 3):
        c = px[i:i+3]
        colors[c] += 1
        if c != b'\x00\x00\x00':
            nonblack += 1
    total = min(len(px) // 3, n)
    print(f"{path}: {w}x{h} pixels={total}")
    print(f"distinct colors: {len(colors)}")
    print(f"non-black: {nonblack} ({100.0 * nonblack / max(total,1):.2f}%)")
    for c, cnt in colors.most_common(8):
        print(f"  {c.hex()} x{cnt}")
    # row profile: which 1/8 bands have content
    band = h // 8
    for b in range(8):
        lo, hi = b * band * w, min((b + 1) * band * w, total)
        seg = px[lo*3:hi*3]
        nz = sum(1 for i in range(0, len(seg) - 2, 3) if seg[i:i+3] != b'\x00\x00\x00')
        print(f"  band {b} (rows {b*band}-{(b+1)*band}): {nz} nonblack")
    return 0

if __name__ == '__main__':
    sys.exit(main(sys.argv[1]))
