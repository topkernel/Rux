#!/bin/bash
# pa115debt1 — Tier-1 debt battery on main@afc41b8b debug bzImage
cd "$(dirname "$0")/.."
STOP=run/pa115debt1-window.stop
rm -f "$STOP"
( while [ ! -f "$STOP" ]; do
    echo "$(date '+%F %T') load1=$(cut -d' ' -f1 /proc/loadavg) qemus=$(pgrep -c qemu-system || echo 0)"
    sleep 60
  done
  echo "$(date '+%F %T') window close: load1=$(cut -d' ' -f1 /proc/loadavg) qemus=$(pgrep -c qemu-system || echo 0)"
) > run/pa115debt1-window.log 2>&1 &
MON=$!
GATE_MAX_FOREIGN=8 run/perf-gate.sh --tiers 1,3 \
    --kernel run/pa115-main-afc41b8b-debug-bzImage --tag pa115debt1 \
    > run/pa115debt1-gate.txt 2>&1
echo "gate rc=$?" >> run/pa115debt1-gate.txt
touch "$STOP"; wait $MON 2>/dev/null
