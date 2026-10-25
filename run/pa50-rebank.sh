#!/bin/bash
# pa50-rebank.sh — quiet-window re-bank of the Tier-1 volume +
# nanosleep rows (leg-35 standing debt). ONE boot, load-gated:
#   pa50main on run/pa50-main-bzImage (PURE main ddb3ed01, x86_64
#   debug, worktree pa50-main) — 780 s syscost boot; a PASSING quiet
#   run re-banks VOLUME_BASE and the nanosleep row in
#   docs/development/perf-gate-baseline.md (both rows are marked
#   "re-bank from a passing quiet-window gate run"; pa46main's
#   load1-11-17 window read 93.2M with a cpu0 tick miss — not quiet).
# Gate: load1 <= 9 AND qemus <= 8 (tighter than the census gate: the
#   point is a genuinely quiet window, comparable to pa38's). Deadline
# 6h. Successor note: if the deadline passed with no window, re-arm
# this script unchanged; parse any boot that ran with
#   perf-gate.sh --skip-boot --log run/pa-pa50main-serial.log
cd /home/william/rux-agents/perf-audit || exit 1
LOG=run/pa50-rebank.log
GATE_LOAD="${GATE_LOAD:-9}"
GATE_QEMU="${GATE_QEMU:-8}"
DEADLINE=$(( $(date +%s) + 6*3600 ))

echo "=== pa50-rebank armed $(date '+%F %T') load-gate=${GATE_LOAD} qemu-gate=${GATE_QEMU} deadline=6h ===" >> "$LOG"
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
  l1=$(awk '{printf "%d", $1}' /proc/loadavg)
  q=$(pgrep -c qemu-system 2>/dev/null || echo 99)
  echo "$(date '+%F %T') gate check: load1=$l1 qemus=$q (need <=${GATE_LOAD}/<=${GATE_QEMU})" >> "$LOG"
  if [ "$l1" -le "$GATE_LOAD" ] && [ "$q" -le "$GATE_QEMU" ]; then
    echo "$(date '+%F %T') window open: re-bank boot pa50main start" >> "$LOG"
    APPEND_EXTRA="syscost=1 sprobe=0" bash run/pa-boot.sh pa50main run/pa50-main-bzImage 780 >> "$LOG" 2>&1
    rc=$?
    echo "$(date '+%F %T') boot pa50main rc=$rc" >> "$LOG"
    echo "=== pa50-rebank complete $(date '+%F %T') rc=$rc ===" >> "$LOG"
    exit $rc
  fi
  sleep 120
done
echo "=== pa50-rebank NO WINDOW within 6h deadline ($(date '+%F %T')) ===" >> "$LOG"
exit 4
