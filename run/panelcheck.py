#!/usr/bin/env python3
"""panelcheck.py — deterministic GNOME-frame verifier (no vision model).

Checks a fire.py PPM shot for the GNOME Shell panel signature by direct
pixel math:
  - a full-width #232323 (±4) bar across rows 0..33 (>=85% occupancy)
  - bright text-glyph pixels (lum > 0x60) inside that strip, clustered
    by x-bucket (left = Activities, center = clock)
  - a white cursor sprite (bright cluster below the strip)

leg-6 additions, for the fuller-workspace gap list:
  - window: a large near-white rectangle below the panel (the xterm
    probe window is pinned -bg white); reported with its bounding box
  - banner: a #232323-ish dark box just under the panel with bright
    glyphs inside (the critical-urgency notify probe is resident)
  - right-side indicator clusters (x-buckets >= 13, i.e. x >= 1040)
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
    win_px = 0; win_bbox = None
    ban_rows = {}; ban_glyphs = 0
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
            # notification banner: dark panel-colored box with glyphs in
            # the band just below the panel, centered region
            if 34 <= y < 120 and 380 <= x < 900:
                if abs(p[0] - 0x23) <= 6 and abs(p[1] - 0x23) <= 6 and abs(p[2] - 0x23) <= 6:
                    ban_rows[y] = ban_rows.get(y, 0) + 1
                if lum > 0x60:
                    ban_glyphs += 1
            # app window: near-white body (xterm probe is -bg white)
            if y >= 60 and p[0] >= 0xE0 and p[1] >= 0xE0 and p[2] >= 0xE0:
                win_px += 1
                if win_bbox is None:
                    win_bbox = [x, y, x, y]
                else:
                    win_bbox[0] = min(win_bbox[0], x); win_bbox[1] = min(win_bbox[1], y)
                    win_bbox[2] = max(win_bbox[2], x); win_bbox[3] = max(win_bbox[3], y)
    occ = strip_ok / strip if strip else 0
    clusters = {k: len(v) for k, v in sorted(glyphs.items()) if len(v) >= 30}
    # right-side status indicators are small sprites (aggregate/power
    # chevron ~15-25px) — report anything >= 12px there separately
    right_clusters = {k: len(v) for k, v in sorted(glyphs.items())
                      if k >= 13 and len(v) >= 12}
    panel = occ >= 0.85 and len(clusters) >= 2
    win = win_px >= 15000 and win_bbox and (win_bbox[2] - win_bbox[0]) >= 300 \
        and (win_bbox[3] - win_bbox[1]) >= 150
    ban_rows_ok = sum(1 for n in ban_rows.values() if n >= 300)
    banner = ban_rows_ok >= 24 and ban_glyphs >= 25
    print(f"{path}: panel={'YES' if panel else 'NO'} bar_occupancy={occ:.2f} "
          f"glyph_clusters(x80px)={clusters} right_side={right_clusters or 'none'} "
          f"cursor_px={len(body_bright)} "
          f"window={'YES bbox=' + str(win_bbox) + ' px=' + str(win_px) if win else 'NO px=' + str(win_px)} "
          f"banner={'YES rows>=300:' + str(ban_rows_ok) + ' glyphs=' + str(ban_glyphs) if banner else 'NO rows=' + str(ban_rows_ok) + ' glyphs=' + str(ban_glyphs)}")
    return panel

if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit("usage: panelcheck.py shot.ppm [...]")
    ok = False
    for p in sys.argv[1:]:
        ok = check(p) or ok
    sys.exit(0 if ok else 1)
