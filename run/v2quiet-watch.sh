#!/bin/bash
# v2quiet-watch.sh — quiet-window runner for the README v2 asset boots.
#
# Session-16 AB adjudication: the merged-tree no-render/Oh-no finding
# is load sensitivity (the pre-merge control fails identically under
# load), so the asset path is a QUIET-host boot. This watcher yields
# to the r26gate perf gate (same quiet window, armed earlier), then
# at the first window with <2 foreign QEMUs it:
#   1. boots v2quiet (v2type-boot.sh BASE_T=815: click 800, echo rux
#      815-831, uname -r 865-881) and runs convcheck-v2 on it;
#   2. if verified, boots v2quiet2 and cross-checks both boots' typed
#      tail geometry (the asset rule: >=2 boots identical line
#      geometry) — compose/README steps stay with the lane session.
# No boot looping: one sequence, then it leaves the log for review.
# Log: run/v2quiet-watch.log.
cd "$(dirname "$0")/.." || exit 1
LOG=run/v2quiet-watch.log
echo "watcher armed $(date)" > "$LOG"
tries=0
while [ "$tries" -lt 120 ]; do          # 10 h at 300 s
  tries=$((tries + 1))
  n=$(pgrep -c qemu-system 2>/dev/null || true)
  n="${n:-0}"
  gate_done=0
  grep -q '^gate rc=' run/perf-gate-r26gate.log 2>/dev/null && gate_done=1
  pgrep -f r26gate-watch.sh >/dev/null 2>&1 || gate_done=1
  if [ "$n" -lt 2 ] && [ "$gate_done" = 1 ]; then
    echo "quiet window at $(date) (foreign=$n) — firing v2quiet boot 1" >> "$LOG"
    bash run/v2type-boot.sh v2quiet 815 >> "$LOG" 2>&1
    echo "v2quiet boot rc=$? at $(date)" >> "$LOG"
    python3 run/convcheck-v2.py --typing-start 815 --tail 6 \
      runs/gnome-v2quiet >> "$LOG" 2>&1
    rc1=$?
    echo "convcheck v2quiet rc=$rc1 at $(date)" >> "$LOG"
    if [ "$rc1" -eq 0 ]; then
      echo "firing v2quiet2 boot for cross-boot geometry" >> "$LOG"
      bash run/v2type-boot.sh v2quiet2 815 >> "$LOG" 2>&1
      echo "v2quiet2 boot rc=$? at $(date)" >> "$LOG"
      python3 run/convcheck-v2.py --typing-start 815 --tail 6 \
        runs/gnome-v2quiet runs/gnome-v2quiet2 >> "$LOG" 2>&1
      echo "convcheck cross rc=$? at $(date)" >> "$LOG"
    fi
    exit 0
  fi
  sleep 300
done
echo "no quiet window in 10h; giving up at $(date)" >> "$LOG"
exit 3
