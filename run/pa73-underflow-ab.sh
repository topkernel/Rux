#!/bin/bash
# pa73: preempt-underflow attribution A/B — direct reproduction of the
# waitid-cluster wedge on both range ends of the suspected main delta.
#
#   A = 0f806d8b (current main tip; carries 488528dc->979fde80 delta)
#   B = 488528dc (pre-delta main; the stress lane's "clean" control base)
#
# Window: sweepstart=1290 .. ~1330 (waitid01..waitid11 = idx 1305-1315,
# the cluster the stress lane's ltp-fix2/r17 logs showed the class in).
# The wedge is intermittent (fleet r64/r65 missed, r66 hit 142x on a
# pre-delta kernel) so each arm gets TWO windows when the first misses.
#
# Verdict rule: class (PREEMPT-HEAL was=0x10001 + PREEMPT-UNDERFLOW
# sub=1 at spinlock guard drop) on ARM B => the delta is exonerated as
# the ORIGIN (the pre-delta fleet v16 logs already show it); absence on
# B with presence on A in the same windows => delta suspicion stands and
# the re-bisect is owed.
#
# QEMU: one per arm, sequential boots inside each arm (drive.py manages
# boot/reboot), -smp 4 -m 2G tcg thread=single (the stress lane's shape).
set -u
cd "$(dirname "$0")/.."

PY=.ltp/drive.py
LIST=.ltp/sweep.list
IMG=/home/william/rux-agents/ltp-fix-b/test/rootfs.img   # snapshot=on, read-only use
OUT=run/pa73-underflow

mkdir -p "$OUT"

run_arm() {  # $1=tag $2=kernel
    local tag="$1" kimg="$2"
    echo "=== arm $tag kernel=$kimg $(date +%H:%M:%S)"
    python3 "$PY" --tag "$tag" --start 1290 --max-wall 1500 \
        --kernel "$kimg" --image "$IMG" --list "$LIST" \
        > "$OUT/drive-$tag.log" 2>&1 &
    echo $! > "$OUT/drive-$tag.pid"
}

case "${1:-}" in
  a) run_arm pa73a run/pa73-main0f8-rux ;;
  b) run_arm pa73b run/pa73-k488-rux ;;
  both)
    run_arm pa73a run/pa73-main0f8-rux
    sleep 5
    run_arm pa73b run/pa73-k488-rux
    ;;
  wait)
    # Block until both drive.py processes exit, then tally.
    while pgrep -F "$OUT/drive-pa73a.pid" >/dev/null 2>&1 || \
          pgrep -F "$OUT/drive-pa73b.pid" >/dev/null 2>&1; do sleep 20; done
    for t in pa73a pa73b; do
        f=".ltp/serial-$t.log"
        echo "== $t: heals=$(grep -c 'PREEMPT-HEAL' "$f" 2>/dev/null || echo 0) underflows=$(grep -c 'PREEMPT-UNDERFLOW' "$f" 2>/dev/null || echo 0) waitid_rows=$(grep -c 'RES|waitid' "$f" 2>/dev/null || echo 0)"
    done
    ;;
  *) echo "usage: $0 {a|b|both|wait}"; exit 1 ;;
esac
