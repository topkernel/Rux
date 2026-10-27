#!/bin/bash
# pa40-decider.sh — leg-31 census-cycle deciding boot, load-gated.
# (Restarted 06:52 after the leg-32 session killed the 06:01 instance
# believing its owner dead — see run/DUAL-SESSION-NOTE-2026-10-25.md.
# Row (b) is CLOSED: in band (+1.8%, pa41-dbg; banked in 53503189).
# Row (a) verdict = pace-coupled, per-op held (pa40 76.6M / pa41gate
# 77.1M under load, census 8/32 exact on both); this boot re-banks the
# volume row + runs the Tier-1 census gate on the NEW main class
# de41d5ab — 6 kernel commits: dfx census-peek v3 + censusat=, KILLDUMP
# task/mm pinning fixes x2, binder stale-async txn reclaim.)
#   Boot: quiet syscost 780s (pa-boot.sh pa44) on run/pa44-main-bzImage.
# Gate before the boot: load1 <= GATE_LOAD (13) AND qemu count <=
#   GATE_QEMU (10). Deadline 4h from arm -> exit 4 (no window).
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

echo "=== pa40-decider (pa44 census boot) armed $(date '+%F %T') load-gate=${GATE_LOAD} qemu-gate=${GATE_QEMU} ===" >> "$LOG"

if wait_gate; then
  echo "$(date '+%F %T') window open: census boot (pa44, main de41d5ab) start" >> "$LOG"
  APPEND_EXTRA="syscost=1 sprobe=0" bash run/pa-boot.sh pa44 \
      run/pa44-main-bzImage 780 >> "$LOG" 2>&1
  echo "$(date '+%F %T') census boot rc=$?" >> "$LOG"
else
  echo "$(date '+%F %T') NO WINDOW within deadline (pa44 census boot)" >> "$LOG"
  exit 4
fi
echo "=== pa40-decider (pa44 census boot) complete $(date '+%F %T') ===" >> "$LOG"
