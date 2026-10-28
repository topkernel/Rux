#!/usr/bin/env python3
"""readme-v2-compose.py — single-canvas README asset for the v2 xterm
conversation (user feedback 2026-10-28: one screenshot with a magnified
detail, not stacked panels).

Takes the conversation-complete shot from a verified fire.py run dir,
decodes the typed-line bands with the SAME deterministic decode as
convcheck-v2.py (no vision model anywhere in this chain), and renders
one canvas:

  - the full desktop frame (1280x800) at the top
  - a 3x NEAREST-neighbor zoom of the conversation region as an inset
    box below it, on a white matte with a colored border
  - a same-color highlight rectangle on the desktop around the exact
    source region, and two connector lines from that rectangle's
    bottom corners to the inset's top corners

All placement is derived from the band decode (the inset frames
exactly the typed lines), so the composition is reproducible from the
raw PPM. No text is drawn — the caption lives in README.

Usage: python3 run/readme-v2-compose.py RUNDIR [SHOT] OUT.png
  SHOT defaults to the run's last shot-*.ppm (same file convcheck-v2
  verified).
"""
import importlib.util
import os
import sys

from PIL import Image, ImageDraw

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location(
    "convcheck_v2", os.path.join(HERE, "convcheck-v2.py"))
cc = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(cc)

HL_COLOR = (255, 127, 0)     # highlight / border / connector color
FRAME_COLOR = (40, 40, 40)   # outer desktop frame
ZOOM = 3                     # inset magnification (pixel-exact NEAREST)
MATTE = 14                   # white matte inside the inset border
BORDER = 3                   # inset border width
GAP = 36                     # desktop bottom -> inset top
CANVAS_W = 1280


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(2)
    rundir, out = sys.argv[1], sys.argv[-1]
    shot = sys.argv[2] if len(sys.argv) > 3 and sys.argv[2] else None
    if shot is None:
        # lexical sort == chronological (fixed-width time stamp)
        shot = sorted(f for f in os.listdir(rundir)
                      if f.startswith("shot-") and f.endswith(".ppm"))[-1]
    path = os.path.join(rundir, shot)
    w, h, img = cc.load_ppm(path)
    assert (w, h) == (1280, 800), f"unexpected shot size {w}x{h}"
    bands = [b for b in cc.line_bands(img, w)
             if not (b[0] <= 116 and b[2] > 4000)]
    tail = bands[-cc.TYPED_TAIL:]
    assert len(tail) == cc.TYPED_TAIL, "not enough typed lines to frame"
    # crop rectangle: typed lines + margin, bounded by the xterm window
    x_lo = min(cc.band_x_extent(img, w, b)[0] or cc.XTERM_X0
               for b in tail)
    x_hi = max(cc.band_x_extent(img, w, b)[1] or cc.XTERM_X0
               for b in tail)
    cx0 = max(0, cc.XTERM_X0 - 8)
    cx1 = min(w - 1, max(x_hi + 28, cc.XTERM_X0 + 360))
    cy0 = max(0, tail[0][0] - 9)
    cy1 = min(h - 1, tail[-1][1] + 9)
    print(f"source rect: ({cx0},{cy0})-({cx1},{cy1}) "
          f"{cx1-cx0+1}x{cy1-cy0+1} from {shot}")
    desk = Image.frombytes("RGB", (w, h), img)
    crop = desk.crop((cx0, cy0, cx1 + 1, cy1 + 1))
    iw, ih = crop.width * ZOOM, crop.height * ZOOM
    inset = crop.resize((iw, ih), Image.NEAREST)
    # canvas
    pad = MATTE + BORDER
    canvas_h = h + GAP + ih + 2 * pad + 6
    canvas = Image.new("RGB", (CANVAS_W, canvas_h), (255, 255, 255))
    canvas.paste(desk, (0, 0))
    d = ImageDraw.Draw(canvas)
    d.rectangle([0, 0, CANVAS_W - 1, h - 1], outline=FRAME_COLOR, width=1)
    # highlight the exact source region on the desktop
    d.rectangle([cx0, cy0, cx1, cy1], outline=HL_COLOR, width=3)
    # inset: border + white matte + zoom
    ix = (CANVAS_W - iw) // 2 - pad
    iy = h + GAP
    d.rectangle([ix, iy, ix + iw + 2 * pad - 1, iy + ih + 2 * pad - 1],
                fill=(255, 255, 255), outline=HL_COLOR, width=BORDER)
    canvas.paste(inset, (ix + pad, iy + pad))
    # connectors: source-rect bottom corners -> inset top corners
    d.line([(cx0, cy1), (ix + BORDER, iy + BORDER)], fill=HL_COLOR, width=2)
    d.line([(cx1, cy1), (ix + iw + 2 * pad - 1 - BORDER, iy + BORDER)],
           fill=HL_COLOR, width=2)
    canvas.save(out, "PNG")
    print(f"wrote {out} ({CANVAS_W}x{canvas_h})")


if __name__ == "__main__":
    main()
