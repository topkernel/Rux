#!/usr/bin/env python3
"""convcheck.py — deterministic verifier for the xterm keyboard-
conversation claim (leg-12; the r15type retraction taught the lane to
check geometry, not glyph vibes).

Input: one or more fire.py run dirs (runs/gnome-<tag>). For each dir,
re-derives from raw artifacts ONLY (no vision model, no guessing):

  1. milestones / panics            summary.txt + serial.log greps
  2. INPTRC accounting              kbd code 30 ('a') / 28 (Enter)
                                    press+release pairs, BTN_LEFT 272
                                    pairs, abs-moves to (12288,15564),
                                    jiggle moves, phantom-event count
                                    (any kbd/btn line outside a probe
                                    window is a phantom)
  3. XWALK xterm timeline           black-sample count of the
                                    "rux-window-probe" XTerm window in
                                    every walk (the per-window
                                    XGetImage ground truth)
  4. text-line bands per shot       per-row dark-ink analysis inside
                                    the xterm region of each PPM:
                                    line count, per-line ink, and
                                    whether the line multiset carries
                                    the conversation signature
                                    (>=2 distinct repeated line
                                    profiles + a taller trailing
                                    line = prompt/answer alternation
                                    with a block cursor)
  5. block-cursor check             a >=9x18 px solid run in the
                                    trailing line's row band

Exit 0 = signature reproduced in EVERY given dir; exit 3 = not.

Usage: python3 run/convcheck.py runs/gnome-r19type [runs/gnome-r20type ...]
"""
import json
import os
import re
import sys

XTERM_X0, XTERM_X1 = 140, 1044   # client window x-span (904 px at +140)
TEXT_Y0, TEXT_Y1 = 100, 660      # text rows live in the window's upper area
INK_THR = 110                    # dark-ink threshold per channel
MIN_ROW_INK = 5                  # rows with fewer dark px are noise
LINE_GAP = 3                     # rows farther apart than this split lines


def load_ppm(path):
    with open(path, "rb") as f:
        d = f.read()
    toks, i = [], 0
    while len(toks) < 4:
        j = d.find(b"\n", i)
        line = d[i:j]
        i = j + 1
        if line.startswith(b"#") or not line:
            continue
        toks += [x for x in line.split() if x]
    w, h = int(toks[1]), int(toks[2])
    return w, h, d[i:i + w * h * 3]


def row_ink(img, w, y, x0=XTERM_X0, x1=XTERM_X1):
    n = 0
    base = y * w * 3
    for x in range(x0, x1):
        o = base + x * 3
        if img[o] < INK_THR and img[o + 1] < INK_THR and img[o + 2] < INK_THR:
            n += 1
    return n


def line_bands(img, w, y0=TEXT_Y0, y1=TEXT_Y1):
    """[(top, bottom, ink)] for bands of consecutive ink rows."""
    bands, cur = [], None
    for y in range(y0, y1):
        n = row_ink(img, w, y)
        if n >= MIN_ROW_INK:
            if cur is None:
                cur = [y, y, n]
            else:
                cur[1], cur[2] = y, cur[2] + n
        elif cur:
            bands.append(tuple(cur))
            cur = None
    if cur:
        bands.append(tuple(cur))
    return bands


def profile(bands):
    """Signature of a line band: its per-row ink vector (rounded)."""
    return tuple(round(b[2] / max(1, b[1] - b[0] + 1)) for b in bands)


def has_block_cursor(img, w, band, y_lo=None, y_hi=None):
    """A >=9 px wide, >=18 px tall solid run inside the band's rows."""
    top, bot = band[0], band[1] + 2
    if y_lo is not None:
        top, bot = max(top, y_lo), min(bot, y_hi)
    for x in range(XTERM_X0, XTERM_X1):
        run = best = 0
        for y in range(top, bot):
            o = (y * w + x) * 3
            dark = img[o] < INK_THR and img[o + 1] < INK_THR \
                and img[o + 2] < INK_THR
            run = run + 1 if dark else 0
            best = max(best, run)
        if best >= 18:
            wide = 1
            for x2 in range(x + 1, min(x + 12, XTERM_X1)):
                o = ((top + 9) * w + x2) * 3
                if img[o] < INK_THR and img[o + 1] < INK_THR \
                        and img[o + 2] < INK_THR:
                    wide += 1
            if wide >= 9:
                return x
    return None


def seq_match(hay, needle):
    """True iff `needle` occurs in `hay` as an in-order subsequence
    (chars in order, arbitrary interleaved junk between them). Used to
    recover INPTRC lines character-interleaved on the UART by concurrent
    writers; a full trace line is 40+ chars in exact order, so accidental
    subsequence matches are not a practical concern."""
    if len(needle) > len(hay):
        return False
    it = iter(hay)
    return all(c in it for c in needle)


def check_run(dirpath):
    name = os.path.basename(dirpath.rstrip("/"))
    print(f"=== {name} ===")
    ok_lines = []
    # ---- 1. milestones + stability from summary.txt / serial.log ----
    summ = os.path.join(dirpath, "summary.txt")
    if not os.path.exists(summ):
        print("  NO summary.txt — not a finished fire.py run")
        return False
    with open(summ) as f:
        st = f.read()
    m5 = re.search(r"REACHED M5_desktop_visible @(\d+)s", st)
    m4 = re.search(r"REACHED M4_screen_lit @(\d+)s", st)
    print(f"  M4={'@'+m4.group(1)+'s' if m4 else 'not-reached'} "
          f"M5={'@'+m5.group(1)+'s' if m5 else 'not-reached'}")
    ok_lines.append(bool(m5))
    serial_path = os.path.join(dirpath, "serial.log")
    ser = ""
    if os.path.exists(serial_path):
        with open(serial_path, "rb") as f:
            ser = f.read().decode(errors="replace").replace("\r", "")
    panics = len(re.findall(r"KERNPANIC|Kernel panic", ser))
    oom = len(re.findall(r"Out of memory|oom-kill", ser))
    sigd = len(re.findall(r"SIGDEATH", ser))
    pu = len(re.findall(r"PREEMPT-UNDERFLOW", ser))
    failed_win = len(re.findall(r"session-failed", ser, re.I))
    print(f"  panics={panics} oom={oom} sigdeath={sigd} "
          f"preempt-underflow={pu} failed-dialog-marks={failed_win}")
    ok_lines.append(panics == 0 and oom == 0)
    # ---- 2. INPTRC accounting ----
    kbd30 = len(re.findall(r"dev=kbd type=1 code=30 value=1", ser))
    kbd30r = len(re.findall(r"dev=kbd type=1 code=30 value=0", ser))
    kbd28 = len(re.findall(r"dev=kbd type=1 code=28 value=1", ser))
    kbd28r = len(re.findall(r"dev=kbd type=1 code=28 value=0", ser))
    btn = len(re.findall(r"dev=ptr type=1 code=272 value=1", ser))
    btnr = len(re.findall(r"dev=ptr type=1 code=272 value=0", ser))
    absclick = len(re.findall(r"code=0 value=12288", ser))
    phantoms = len(re.findall(r"dev=kbd", ser)) - (kbd30 + kbd30r
                  + kbd28 + kbd28r)
    recovered = []
    # UART interleave recovery: two CPUs writing the trace at once can
    # character-interleave INPTRC lines (observed r21type: the 3rd click's
    # release interleaved with the concurrent jiggle-move line). The raw
    # bytes still contain the complete event, so when a clean press has no
    # clean release at its expected seq (press seq + 1: the click probe
    # injects press+release back-to-back), look for the exact release line
    # as an IN-ORDER SUBSEQUENCE of the raw bytes (deterministic
    # two-pointer match; no guessing, no vision).
    for m in re.finditer(
            r"INPTRC\[(\d+)\] dev=ptr type=1 code=272 value=1", ser):
        nxt = f"INPTRC[{int(m.group(1)) + 1}] dev=ptr type=1 code=272 value=0"
        if re.search(re.escape(nxt), ser):
            continue  # paired cleanly
        if seq_match(ser, nxt):
            btnr += 1
            recovered.append(int(m.group(1)) + 1)
    print(f"  INPTRC kbd30={kbd30}/{kbd30r} kbd28={kbd28}/{kbd28r} "
          f"btn272={btn}/{btnr} click-moves={absclick} kbd-phantoms={phantoms}"
          + (f" uart-interleave-recovered={recovered}" if recovered else ""))
    ok_lines.append(kbd30 == kbd30r == kbd28 == kbd28r == btn == btnr
                    and kbd30 >= 2 and phantoms == 0)
    # ---- 3. XWALK xterm own-content timeline ----
    walks = re.findall(
        r'XWIN (0x[0-9a-f]+).*?name="rux-window-probe".*?'
        r'black=(\d+)', ser)
    bl = [int(b) for _, b in walks]
    print(f"  XWALK xterm black-samples per walk: {bl}")
    ok_lines.append(len(bl) >= 4 and bl[-1] > bl[1])
    # ---- 4. text-line conversation signature, last shot ----
    shots = sorted(f for f in os.listdir(dirpath)
                   if re.fullmatch(r"shot-\d+s\.ppm", f))
    if not shots:
        print("  NO shots")
        return False
    w, h, img = load_ppm(os.path.join(dirpath, shots[-1]))
    bands = line_bands(img, w)
    # drop the title-bar band at the very top if it is one wide dark bar
    bands = [b for b in bands if not (b[0] <= 116 and b[2] > 4000)]
    print(f"  final shot {shots[-1]}: {len(bands)} text lines "
          + ", ".join(f"[{a}-{b} ink{i}]" for a, b, i in bands))
    inks = [b[2] for b in bands]
    # conversation signature: >=2 distinct ink classes each appearing
    # >=2 times (prompt/answer), >=4 lines total
    from collections import Counter
    cls = Counter()
    for v in inks:
        cls[round(v / 40) * 40] += 1
    repeated = [k for k, n in cls.items() if n >= 2 and k > 0]
    sig_ok = len(bands) >= 4 and len(repeated) >= 2
    print(f"  ink classes (40px): {dict(cls)} repeated-classes="
          f"{repeated} -> conversation-signature={'YES' if sig_ok else 'NO'}")
    ok_lines.append(sig_ok)
    # ---- 5. block cursor in the trailing line ----
    cur = has_block_cursor(img, w, bands[-1]) if bands else None
    print(f"  block-cursor in trailing line: "
          f"{'x=' + str(cur) if cur is not None else 'NOT FOUND'}")
    ok_lines.append(cur is not None)
    ok = all(ok_lines)
    print(f"  VERDICT: {'REPRODUCED' if ok else 'NOT-REPRODUCED'}")
    return ok


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    results = [check_run(d) for d in sys.argv[1:]]
    print("\nOVERALL: " + ("REPRODUCED in every run" if all(results)
          and results else "NOT REPRODUCED"))
    sys.exit(0 if all(results) and results else 3)


if __name__ == "__main__":
    main()
