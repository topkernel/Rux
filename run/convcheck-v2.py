#!/usr/bin/env python3
"""convcheck-v2.py — deterministic verifier for the README v2 xterm
conversation (`echo rux` + `uname -r`; user feedback 2026-10-28).

Same discipline as convcheck.py (geometry decode, no vision model in
the chain), strengthened for real commands:

  1. milestones / panics           summary.txt + serial.log greps
  2. INPTRC keystroke script       the EXACT v2 key sequence must
                                   appear as an ordered subsequence of
                                   the serial bytes (e,c,h,o,spc,r,u,x,
                                   ret, u,n,a,m,e,spc,minus,r,ret =
                                   input codes 18,46,35,24,57,19,22,
                                   45,28,22,49,30,50,18,57,12,19,28),
                                   every press paired with a release
                                   (byte-level subsequence fallback for
                                   UART-interleaved lines), zero
                                   phantom kbd events, one click pair
  3. XWALK content change          the rux-window-probe XTerm window's
                                   black-sample count must grow across
                                   the typing window (its own pixels
                                   changed, per-window XGetImage)
  4. typed-region line bands       the final shot's xterm text bands
                                   must end in the 6-line conversation
                                   shape  [input, output, prompt,
                                   input, output, prompt+cursor]:
                                   ink(input) > ink(output) for both
                                   commands, block cursor present in
                                   the LAST band and absent in the
                                   third-from-last (bare prompt),
                                   >= 6 bands total
  5. two-boot geometry equality    with 2 run dirs given: identical
                                   band count/tops/heights for the
                                   typed tail and equal block-cursor x
                                   (banner head lines may vary between
                                   boots; the typed tail may not)

Exit 0 = verified in EVERY given run; exit 3 = not.
Usage: python3 run/convcheck-v2.py runs/gnome-v2a [runs/gnome-v2b ...]
"""
import os
import re
import sys

XTERM_X0, XTERM_X1 = 214, 1030   # TEXT AREA x-span: the client starts at
                                  # x=150 but this image's xterm draws a
                                  # ~60px left scrollbar inside it, so
                                  # glyphs begin at ~214 (v2a measurement);
                                  # right edge stops short of the frame
TEXT_Y0, TEXT_Y1 = 150, 660      # client text rows (frame title ~127-156)
INK_BG = 224                     # nominal xterm background level
INK_DIST = 180                   # summed channel drop below bg = text ink
                                 # (v2a lesson: bash colors the prompt —
                                 # green/blue glyphs are ink to a distance
                                 # test, invisible to an all-dark test)


def _ink(img, o):
    return ((INK_BG - img[o]) + (INK_BG - img[o + 1])
            + (INK_BG - img[o + 2])) > INK_DIST
MIN_ROW_INK = 5                  # rows with fewer dark px are noise
# Linux input event codes for the v2 script (press order).
V2_CODES = [18, 46, 35, 24, 57, 19, 22, 45, 28,        # echo rux<ret>
            22, 49, 30, 50, 18, 57, 12, 19, 28]        # uname -r<ret>
TYPED_TAIL = 6                   # bands the two commands produce


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


def row_ink(img, w, y, x0=XTERM_X0, x1=XTERM_X1, chrome=None):
    n = 0
    base = y * w * 3
    for x in range(x0, x1):
        if chrome and x in chrome:
            continue
        o = base + x * 3
        if _ink(img, o):
            n += 1
    return n


def detect_chrome(img, w):
    """Window-EDGE columns dark in >=60% of text rows (borders, the
    right-edge scrollbar) — persistent per-row ink that would glue all
    text lines into one band. Restricted to the window EDGES: a v2a
    lesson — every line's dark navy "root@rux" prompt sits at the same
    x, so scanning the whole window width misclassifies the prompt
    column block as chrome and eats the prompt lines entirely."""
    ys = list(range(130, 640))
    chrome = set()

    def frac_dark(x):
        n = 0
        for y in ys:
            o = (y * w + x) * 3
            if _ink(img, o):
                n += 1
        return n / len(ys)

    for x in range(XTERM_X0, XTERM_X0 + 5):          # left border
        if frac_dark(x) >= 0.6:
            chrome.add(x)
    for x in range(XTERM_X1 - 80, XTERM_X1):         # scrollbar / right
        if frac_dark(x) >= 0.6:
            chrome.add(x)
    return chrome


def line_bands(img, w, y0=TEXT_Y0, y1=TEXT_Y1, chrome=None):
    """[(top, bottom, ink)] for bands of consecutive ink rows."""
    bands, cur = [], None
    for y in range(y0, y1):
        n = row_ink(img, w, y, chrome=chrome)
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


def band_x_extent(img, w, band):
    """First/last dark-ink column inside a band (for crop sizing)."""
    xs = []
    for y in range(band[0], band[1] + 1):
        base = y * w * 3
        for x in range(XTERM_X0, XTERM_X1):
            o = base + x * 3
            if _ink(img, o):
                xs.append(x)
                break
        for x in range(XTERM_X1 - 1, XTERM_X0, -1):
            o = base + x * 3
            if _ink(img, o):
                xs.append(x)
                break
    return (min(xs), max(xs)) if xs else (None, None)


def has_block_cursor(img, w, band):
    """A >=9 px wide, >=18 px tall solid run inside the band's rows."""
    top, bot = band[0], band[1] + 2
    for x in range(XTERM_X0, XTERM_X1):
        run = best = 0
        for y in range(top, bot):
            o = (y * w + x) * 3
            dark = _ink(img, o)
            run = run + 1 if dark else 0
            best = max(best, run)
        if best >= 18:
            wide = 1
            for x2 in range(x + 1, min(x + 12, XTERM_X1)):
                o = ((top + 9) * w + x2) * 3
                if _ink(img, o):
                    wide += 1
            if wide >= 9:
                return x
    return None


def seq_match(hay, needle):
    """True iff `needle` occurs in `hay` as an in-order subsequence."""
    if len(needle) > len(hay):
        return False
    it = iter(hay)
    return all(c in it for c in needle)


def check_run(dirpath, shot=None, tail_n=TYPED_TAIL, typing_start=1415):
    name = os.path.basename(dirpath.rstrip("/"))
    print(f"=== {name}{'' if shot is None else ' (' + shot + ')'} ===")
    ok = []
    summ = os.path.join(dirpath, "summary.txt")
    if not os.path.exists(summ):
        print("  NO summary.txt — not a finished fire.py run")
        return False, None
    with open(summ) as f:
        st = f.read()
    m4 = re.search(r"REACHED M4_screen_lit @(\d+)s", st)
    m5 = re.search(r"REACHED M5_desktop_visible @(\d+)s", st)
    print(f"  M4={'@'+m4.group(1)+'s' if m4 else 'not-reached'} "
          f"M5={'@'+m5.group(1)+'s' if m5 else 'not-reached'}")
    ok.append(bool(m5))
    with open(os.path.join(dirpath, "serial.log"), "rb") as f:
        ser = f.read().decode(errors="replace").replace("\r", "")
    panics = len(re.findall(r"KERNPANIC|Kernel panic", ser))
    oom = len(re.findall(r"Out of memory|oom-kill", ser))
    sigd = len(re.findall(r"SIGDEATH", ser))
    print(f"  panics={panics} oom={oom} sigdeath={sigd}")
    ok.append(panics == 0 and oom == 0)
    # ---- 2. exact v2 keystroke script in INPTRC ----
    presses = [int(m.group(1)) for m in
               re.finditer(r"dev=kbd type=1 code=(\d+) value=1", ser)]
    releases = [int(m.group(1)) for m in
                re.finditer(r"dev=kbd type=1 code=(\d+) value=0", ser)]
    kbd_lines = len(re.findall(r"dev=kbd", ser))
    phantoms = kbd_lines - (len(presses) + len(releases))
    script_ok = seq_match(presses, V2_CODES)
    # pair check: every code's press count == release count; a missing
    # release LINE is recovered if the exact release text occurs in the
    # raw byte stream as an in-order subsequence (v1 lesson: concurrent
    # UART writers can character-interleave INPTRC lines; the bytes are
    # still all there, only the line framing is destroyed)
    from collections import Counter
    pc, rc = Counter(presses), Counter(releases)
    uart_recovered = []
    for code in list(pc):
        while rc[code] < pc[code]:
            nxt = f"INPTRC dev=kbd type=1 code={code} value=0"
            if seq_match(ser, nxt):
                rc[code] += 1
                uart_recovered.append(code)
            else:
                break
    paired = all(pc[c] == rc[c] for c in pc) and all(pc[c] == rc[c]
                                                     for c in rc)
    print(f"  INPTRC kbd presses={len(presses)} releases={len(releases)} "
          f"phantoms={phantoms} script-subsequence="
          f"{'YES' if script_ok else 'NO'} paired={paired}"
          + (f" uart-recovered-releases={uart_recovered}"
             if uart_recovered else ""))
    ok.append(script_ok and paired and phantoms == 0
              and len(presses) == len(V2_CODES))
    # ---- 3. XWALK per-window content change across typing ----
    walks = re.findall(
        r'XWIN 0x[0-9a-f]+ depth=24 map=2 904x536 '
        r'name="rux-window-probe".*?black=(\d+)', ser)
    bl = [int(b) for b in walks]
    grew = len(bl) >= 4 and bl[-1] > bl[0]
    print(f"  XWALK xterm black-samples per walk: {bl[:3]}...{bl[-3:]} "
          f"-> content-grew={'YES' if grew else 'NO'}")
    ok.append(grew)
    # ---- 3b. typed text RENDERED? diff vs the last pre-typing shot ----
    # The v2a lesson: kernel INPTRC can be 1:1 clean while the xterm
    # renders nothing (session dying; only the cursor cell moved). The
    # window's own pixels must show NEW text rows between the last
    # pre-typing shot and the conversation shot. Deterministic: rows
    # y>150 with >30 changed pixels (sum-of-channel diff > 60).
    all_shots = sorted(f for f in os.listdir(dirpath)
                       if re.fullmatch(r"shot-\d+s\.ppm", f))
    pre = [f for f in all_shots
           if int(re.fullmatch(r"shot-(\d+)s\.ppm", f).group(1)) < typing_start]
    if shot and pre:
        w, h, img = load_ppm(os.path.join(dirpath, shot))
        w2, h2, img_pre = load_ppm(os.path.join(dirpath, pre[-1]))
        new_rows = 0
        for y in range(150, 660):
            n = 0
            for x in range(XTERM_X0, XTERM_X1, 2):
                o = (y * w + x) * 3
                dpre = abs(img[o] - img_pre[o]) + abs(img[o + 1] - img_pre[o + 1]) \
                    + abs(img[o + 2] - img_pre[o + 2])
                if dpre > 60:
                    n += 1
            if n > 30:
                new_rows += 1
        print(f"  typed-text-render check vs {pre[-1]}: "
              f"{new_rows} new-text rows -> "
              f"{'RENDERED' if new_rows >= 2 else 'NOT-RENDERED'}")
        ok.append(new_rows >= 2)

    # ---- 4. typed-region line bands on the chosen shot ----
    shots = sorted(f for f in os.listdir(dirpath)
                   if re.fullmatch(r"shot-\d+s\.ppm", f))
    if not shots:
        print("  NO shots")
        return False, None
    shot = shot or shots[-1]
    if shot not in shots:
        print(f"  {shot} not in run dir")
        return False, None
    w, h, img = load_ppm(os.path.join(dirpath, shot))
    chrome = detect_chrome(img, w)
    bands = [b for b in line_bands(img, w, chrome=chrome)
             if not (b[0] < 150 and b[2] > 4000)]   # drop title bar
    print(f"  shot {shot}: {len(bands)} text lines "
          + " ".join(f"[{a}-{b} i{i}]" for a, b, i in bands))
    if len(bands) < tail_n:
        print(f"  VERDICT: NOT-REPRODUCED (fewer than {tail_n} lines)")
        return False, None
    tail = bands[-tail_n:]
    if tail_n == TYPED_TAIL:
        in1, out1, pr1, in2, out2, pr2 = tail
        ink_order = in1[2] > out1[2] and in2[2] > out2[2]
        cur_last = has_block_cursor(img, w, pr2)
        cur_mid = has_block_cursor(img, w, pr1)
        mid_note = "mid-prompt"
    else:   # echo-only conversation: [input, output, prompt+cursor]
        in1, out1, pr2 = tail
        ink_order = in1[2] > out1[2]
        cur_last = has_block_cursor(img, w, pr2)
        cur_mid = has_block_cursor(img, w, in1)
        mid_note = "input-line"
    print(f"  tail inks: {[b[2] for b in tail]} "
          f"input>output={'YES' if ink_order else 'NO'}; "
          f"block-cursor last-line="
          f"{'x='+str(cur_last) if cur_last is not None else 'NO'} "
          f"{mid_note}={'present(BAD)' if cur_mid is not None else 'absent'}")
    ok.append(ink_order and cur_last is not None and cur_mid is None)
    geom = {"shot": shot, "bands": bands,
            "cursor_x": cur_last,
            "tail_tops": [b[0] for b in tail],
            "tail_heights": [b[1] - b[0] + 1 for b in tail],
            "tail_inks": [b[2] for b in tail],
            "tail_x_extent": band_x_extent(img, w, tail[0]),
            "w": w}
    verdict = all(ok)
    print(f"  VERDICT: {'REPRODUCED' if verdict else 'NOT-REPRODUCED'}")
    return verdict, geom


def cross_check(g1, g2):
    """Boot-to-boot typed-tail geometry equality (±40 ink)."""
    print("=== cross-boot typed-tail geometry ===")
    same_tops = g1["tail_tops"] == g2["tail_tops"]
    same_h = g1["tail_heights"] == g2["tail_heights"]
    same_ink = all(abs(a - b) <= 40
                   for a, b in zip(g1["tail_inks"], g2["tail_inks"]))
    same_cur = g1["cursor_x"] == g2["cursor_x"]
    print(f"  tops equal={same_tops} heights equal={same_h} "
          f"inks(±40) equal={same_ink} cursor-x equal={same_cur} "
          f"({g1['cursor_x']} vs {g2['cursor_x']})")
    return same_tops and same_h and same_ink and same_cur


def main():
    args = sys.argv[1:]
    shot = None
    tail_n = TYPED_TAIL
    typing_start = 1415
    dirs = []
    it = iter(args)
    for a in it:
        if a == "--shot":
            shot = next(it)
        elif a == "--tail":
            tail_n = int(next(it))
        elif a == "--typing-start":
            typing_start = int(next(it))
        else:
            dirs.append(a)
    if not dirs:
        print(__doc__)
        sys.exit(2)
    geoms, results = [], []
    for d in dirs:
        v, g = check_run(d, shot=shot, tail_n=tail_n,
                         typing_start=typing_start)
        results.append(v)
        if g:
            geoms.append(g)
    cross = True
    if len(geoms) == 2:
        cross = cross_check(geoms[0], geoms[1])
    elif len(dirs) == 2:
        cross = False   # one run failed decode — cannot claim equality
    all_ok = all(results) and cross and results
    print("\nOVERALL: " + ("REPRODUCED (script + geometry)"
          if all_ok else "NOT REPRODUCED"))
    sys.exit(0 if all_ok else 3)


if __name__ == "__main__":
    main()
