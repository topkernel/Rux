#!/bin/bash
# r26gate-watch.sh — patient auto-retry for the R86-merge perf gate
# (Tier 1 syscost + Tier 2 pbench, owed for a384048c on the merged tree).
#
# The predecessor's 27-min watcher gave up ("foreign count never < 2").
# This one polls every 300 s for up to 6 h and fires the gate the first
# time fewer than 2 foreign QEMUs are up. If another lane grabs the
# capacity in between (gate exits 3 on its own guard), it keeps waiting
# instead of exiting. Runs detached; log in run/perf-gate-r26gate.log.
cd "$(dirname "$0")/.." || exit 1
LOG=run/perf-gate-r26gate.log
echo "watcher armed $(date)" > "$LOG"
tries=0
while [ "$tries" -lt 72 ]; do
  tries=$((tries + 1))
  n=$(pgrep -c qemu-system 2>/dev/null || echo 0)
  if [ "${n:-0}" -lt 2 ]; then
    echo "quiet window at $(date) (foreign=$n) — firing gate (attempt $tries)" >> "$LOG"
    run/perf-gate.sh --tiers 1,2 --tag r26gate >> "$LOG" 2>&1
    rc=$?
    echo "gate rc=$rc at $(date)" >> "$LOG"
    # rc 0 = green; rc 3 = capacity guard raced us, keep waiting;
    # any other rc = the gate ran and judged — stop and leave the log.
    [ "$rc" -ne 3 ] && exit "$rc"
  fi
  sleep 300
done
echo "no quiet window in 6h; giving up at $(date)" >> "$LOG"
exit 3
