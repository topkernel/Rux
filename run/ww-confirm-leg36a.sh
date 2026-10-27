#!/bin/bash
# ww-confirm-leg36a.sh — wake-placement confirmator (one boot per side).
#
# Boots the pbench image with dfx=wwcensus so the WW heartbeat carries
# the hint=onor/steal/exp counters, and slices them over the p5 window
# (PBENCH p5a_pipe .. PBENCH PREP tree) — the pipe/socket ping-pong
# phases where the placement race lives.
#
#   SIDE=skip   (default) the fix active (skip on)
#   SIDE=noskip wresv=0 — same instrument, pick-side skip disabled
#
# Usage: SIDE=skip   run/ww-confirm-leg36a.sh [bzImage]
#        SIDE=noskip run/ww-confirm-leg36a.sh [bzImage]
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SIDE="${SIDE:-skip}"
BZ="${1:-$ROOT/target/x86_64-unknown-none/debug/bzImage}"
TAG="leg36a-ww-${SIDE}"
if [ "$SIDE" = "noskip" ]; then
    EXTRA="dfx=wwcensus wresv=0"
else
    EXTRA="dfx=wwcensus"
fi
SERIAL="$ROOT/work/pbench-${TAG}-serial.log"

# pbench_boot.py: ACCEL env overrides the default tcg,thread=multi (the
# thread=single regime makes the idle-vCPU delay structural — the
# deterministic-steal regime the reservation exists to close; use it for
# the A/B pair when the host is not oversubscribed).
# P5 rows land well before the full battery; 600 s is enough and keeps
# the QEMU window short (host discipline).
python3 "$ROOT/test/pbench_boot.py" --bzimage "$BZ" --tag "$TAG" \
    --timeout 600 --append-extra "$EXTRA" \
    ${ACCEL:+--accel "$ACCEL"} \
    > "$ROOT/run/${TAG}-driver.log" 2>&1
rc=$?
echo "boot rc=$rc serial=$SERIAL"

python3 - "$SERIAL" "$SIDE" <<'EOF'
import re, sys
path, side = sys.argv[1], sys.argv[2]
lines = open(path, errors="replace").read().splitlines()
# WW counter snapshots: list of (t_or_None, onor, steal, exp)
ww = []
for ln in lines:
    m = re.search(r"WW: wakes=(\d+) picks=(\d+) avg_run=\d+ms stranded=\d+ "
                  r"strand_max=\d+ms hint=onor=(\d+) steal=(\d+) exp=(\d+)", ln)
    if m:
        ww.append((int(m.group(1)), int(m.group(2)),
                   int(m.group(3)), int(m.group(4)), int(m.group(5))))
# locate the p5 window boundaries by serial line index
i_p5a = i_prep = None
for i, ln in enumerate(lines):
    if "PBENCH p5a_pipe" in ln and i_p5a is None:
        i_p5a = i
    if "PBENCH PREP" in ln and i_prep is None:
        i_prep = i
def last_ww_before(idx):
    best = None
    for i, ln in enumerate(lines[:idx]):
        if "WW: wakes=" in ln:
            best = ln
    return best
def parse(ln):
    m = re.search(r"hint=onor=(\d+) steal=(\d+) exp=(\d+)", ln or "")
    return tuple(int(x) for x in m.groups()) if m else None
base = parse(last_ww_before(i_p5a or 0))
# last WW line in window (or last overall if no PREP yet)
end_idx = i_prep if i_prep else len(lines)
tail = None
for ln in lines[:end_idx]:
    if "WW: wakes=" in ln:
        tail = ln
end = parse(tail)
print("side=%s p5_window_lines=%s..%s" % (side, i_p5a, i_prep))
if base and end:
    donor = end[0]-base[0]; dsteal = end[1]-base[1]; dexp = end[2]-base[2]
    tot = donor + dsteal
    print("p5-window deltas: onor=%d steal=%d exp=%d (onor+steal=%d, onor share=%.1f%%)"
          % (donor, dsteal, dexp, tot, 100.0*donor/tot if tot else 0.0))
elif end:
    print("tail counters (no baseline line before p5a): onor=%d steal=%d exp=%d" % end)
else:
    print("NO WW heartbeat lines found — wwcensus not active?")
# also dump any PREEMPT-UNDERFLOW / KERNPANIC sightings (mission-3 watch)
for pat in ("PREEMPT-UNDERFLOW", "KERNPANIC", "WAKE-STRANDED"):
    n = sum(1 for ln in lines if pat in ln)
    print("%s count=%d" % (pat, n))
EOF
