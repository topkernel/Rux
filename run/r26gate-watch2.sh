#!/bin/bash
# r26gate-watch2.sh — re-arm of the R86 perf-gate watcher (first window
# expired 00:14:30 with no quiet window). Boots the CLEAN kernel stash
# (work/rux-r86gate-clean-kernel = the 7b194864-era build: merged tree +
# netlink/cgroup2-statfs login fixes, WITHOUT the gdm hunt's diagnostic
# prints — tier numbers must not carry debug console output).
cd "$(dirname "$0")/.." || exit 1
LOG=run/perf-gate-r26gate.log
echo "watcher re-armed $(date) (clean-kernel stash)" >> "$LOG"
tries=0
while [ "$tries" -lt 72 ]; do
  tries=$((tries + 1))
  n=$(pgrep -c qemu-system 2>/dev/null || true)
  n="${n:-0}"
  if [ "$n" -lt 2 ]; then
    echo "quiet window at $(date) (foreign=$n) — firing gate (attempt $tries)" >> "$LOG"
    run/perf-gate.sh --tiers 1,2 --tag r26gate --kernel "$PWD/work/rux-r86gate-clean-kernel" >> "$LOG" 2>&1
    rc=$?
    echo "gate rc=$rc at $(date)" >> "$LOG"
    [ "$rc" -ne 3 ] && exit "$rc"
  fi
  sleep 300
done
echo "no quiet window in 6h; giving up at $(date)" >> "$LOG"
exit 3
