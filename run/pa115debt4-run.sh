#!/bin/bash
# pa115debt4 — Tier-2 rerun board (rerun-once rule) for the p4_fork_exit
# (+38%) / p4_fork_exec (+231%) rows; second sample for the fs-family
# re-bank cells.
cd "$(dirname "$0")/.."
STOP=run/pa115debt4-window.stop
rm -f "$STOP"
( while [ ! -f "$STOP" ]; do
    echo "$(date '+%F %T') load1=$(cut -d' ' -f1 /proc/loadavg) qemus=$(pgrep -c qemu-system || echo 0)"
    sleep 60
  done ) > run/pa115debt4-window.log 2>&1 &
MON=$!
GATE_MAX_FOREIGN=8 run/perf-gate.sh --tiers 2 \
    --kernel run/pa115-main-afc41b8b-debug-bzImage --tag pa115debt4 \
    --pbench-baseline work/pbench-pa115debt2-pb-serial.log \
    > run/pa115debt4-gate.txt 2>&1
echo "gate rc=$?" >> run/pa115debt4-gate.txt
touch "$STOP"; wait $MON 2>/dev/null
