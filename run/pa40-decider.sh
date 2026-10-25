#!/bin/bash
# pa40-decider.sh — leg-31 gate-row deciding runs, load-gated.
#   Row (b): pbench TCG quiet boot (test/pbench_boot.py) on the
#            main-class kernel run/pa40-main-bzImage; p6_unlink med vs
#            pb1 32815.925us +-20% (26252.7-39379.1) — flake-rule rerun.
#   Row (a): quiet syscost 780s boot (pa-boot.sh pa40b) same kernel;
#            volume vs pa38 136.4M +-10% + census/tick/per-op rows.
#            (First read pa40: 76.6M under load ~19 with a constant -40%
#            pace deficit at every checkpoint incl. pre-app phases —
#            host-load-dominated, not a decider; per-op rows held.)
# Gate before EACH boot: load1 <= GATE_LOAD (13) AND qemu count <=
#   GATE_QEMU (10). Deadline 4h from arm -> exit 4/5 (no window; verdict
#   then rests on per-op holds + series-spread re-attribution).
# One QEMU at a time; results to run/pa40-decider.log.
cd /home/william/rux-agents/perf-audit || exit 1
LOG=run/pa40-decider.log
GATE_LOAD="${GATE_LOAD:-13}"
GATE_QEMU="${GATE_QEMU:-10}"
DEADLINE=$(( $(date +%s) + 4*3600 ))

gate_open() {
  local l1 q
  l1=$(awk '{printf "%d", $1}' /proc/loadavg)
  q=$(pgrep -c qemu-system 2>/dev/null || echo 99)
  echo "$(date '+%F %T') gate check: load1=$l1 qemus=$q (need <=${GATE_LOAD}/<=${GATE_QEMU})" >> "$LOG"
  [ "$l1" -le "$GATE_LOAD" ] && [ "$q" -le "$GATE_QEMU" ]
}

wait_gate() {
  while [ "$(date +%s)" -lt "$DEADLINE" ]; do
    gate_open && return 0
    sleep 120
  done
  return 1
}

echo "=== pa40-decider armed $(date '+%F %T') load-gate=${GATE_LOAD} qemu-gate=${GATE_QEMU} ===" >> "$LOG"

if wait_gate; then
  echo "$(date '+%F %T') window open: row (b) pbench boot start" >> "$LOG"
  python3 test/pbench_boot.py --bzimage run/pa40-main-bzImage \
      --tag pa40t2 --timeout 1200 >> "$LOG" 2>&1
  echo "$(date '+%F %T') row (b) pbench rc=$?" >> "$LOG"
else
  echo "$(date '+%F %T') NO WINDOW within deadline (row b)" >> "$LOG"
  exit 4
fi

if wait_gate; then
  echo "$(date '+%F %T') window open: row (a) syscost rerun start" >> "$LOG"
  APPEND_EXTRA="syscost=1 sprobe=0" bash run/pa-boot.sh pa40b \
      run/pa40-main-bzImage 780 >> "$LOG" 2>&1
  echo "$(date '+%F %T') row (a) syscost rc=$?" >> "$LOG"
else
  echo "$(date '+%F %T') NO WINDOW within deadline (row a rerun)" >> "$LOG"
  exit 5
fi
echo "=== pa40-decider complete $(date '+%F %T') ===" >> "$LOG"
