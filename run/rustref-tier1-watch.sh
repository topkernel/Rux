#!/bin/bash
# rustref-tier1-watch.sh — armed Tier-1 syscost boot for the refactor
# lane (feature/rust-refactor, head bfccc3d6). Waits for a quiet host
# window (foreign QEMUs <= 1 and load1 <= 10 — the pa46-decider class,
# stricter first attempt so a clean parse is likely), then runs
# run/perf-gate.sh --tiers 1 against the branch's x86_64 bzImage.
# Deadline: 8 h from arm time; if no window opens, exits 3 leaving this
# log as the record. Detached (setsid) per the long-scan discipline.
set -u
ROOT="/home/william/rux-agents/rust-refactor"
LOG="$ROOT/run/perf-gate-rustref-watch.log"
DEADLINE=$(( $(date +%s) + 8*3600 ))

echo "[watch $(date '+%m%d %H:%M:%S')] armed: waiting for qemus<=1 load1<=10, deadline 8h" >> "$LOG"
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
    q=$(pgrep -c qemu-system 2>/dev/null || echo 99)
    l=$(awk '{int($1)>int($1)?0:0; print int($1)}' /proc/loadavg 2>/dev/null || echo 99)
    if [ "${q:-99}" -le 1 ] && [ "${l:-99}" -le 10 ]; then
        echo "[watch $(date '+%m%d %H:%M:%S')] window open (qemus=$q load1=$l) — running Tier 1" >> "$LOG"
        cd "$ROOT" && bash run/perf-gate.sh --tiers 1 --tag rustref >> "$LOG" 2>&1
        rc=$?
        echo "[watch $(date '+%m%d %H:%M:%S')] perf-gate rc=$rc" >> "$LOG"
        exit $rc
    fi
    sleep 300
done
echo "[watch $(date '+%m%d %H:%M:%S')] deadline passed, no window" >> "$LOG"
exit 3
