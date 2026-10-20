#!/bin/bash
# ww-chain.sh — overnight wildwrite hunt chain: repeat ww-hunt.py boots
# (45 min each, watchpoints armed every boot) until a stop file appears.
# Any WW-HIT line in a run's gdb log is the capture — the chain keeps the
# logs (ww-s2-rN-*) for symbolization; it never touches other processes.
#
# Usage: ww-chain.sh <chain-tag> [runs] [gdbport]
#   chain-tag  tag prefix (logs: run/<chain-tag>-rN-*)
#   runs       number of boots (default 100; stop file also ends it)
#   gdbport    base gdbstub port (default 12371)
#
# Stop: touch run/<chain-tag>.stop
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TAG="${1:-ww-s2}"
RUNS="${2:-100}"
PORT="${3:-12371}"
STOP="$ROOT/run/$TAG.stop"
rm -f "$STOP"

i=1
while [ "$i" -le "$RUNS" ]; do
    [ -f "$STOP" ] && { echo "[chain] stop file, ending after $((i-1)) runs"; break; }
    echo "[chain] $(date +%H:%M:%S) run $i/$RUNS starting (port $PORT)" >&2
    python3 "$ROOT/run/ww-hunt.py" "${TAG}-r${i}" 2700 "$PORT" >&2 || {
        echo "[chain] run $i failed rc=$? — continuing" >&2
    }
    # Any watchpoint hit in this run? Report it loudly (capture protocol:
    # symbolize run/ww-*-rN-gdb.log WW-HIT rips with addr2line).
    G="$ROOT/run/${TAG}-r${i}-gdb.log"
    if [ -f "$G" ] && grep -q "WW-HIT" "$G"; then
        echo "[chain] WW-HIT in run $i — CAPTURE. gdb log: $G" >&2
        grep "WW-HIT" "$G" | head -5 >&2
        touch "$STOP"
    fi
    i=$((i+1))
done
echo "[chain] done: $((i-1)) runs" >&2
