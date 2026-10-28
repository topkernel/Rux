#!/bin/bash
# pa80-pair.sh — leg 49 track-1: R37 timer slot-table A/B (sched/timer
# domain).
#   M = run/pa78-branch-strong-bzImage (f15b9660 + lazy revoke, d4addcfc)
#   B = run/pa80-timer-strong-bzImage   (M + R37 slot table, 7ef5a170)
# The branch delta vs M is EXACTLY the timer file, so this isolates the
# slot-table change. Gate (Tier-2): P1-P7 medians within +-20% of M or
# improved; p7_sleep1ms is the target row (one timer arm per sleep —
# the BTreeMap alloc/dealloc pair per arm is gone). p2_ctxsw judged
# against the strong anchors (quiet 1.37, loaded ceiling ~2.0).
# Discipline: 1 lane QEMU at a time, own PIDs only, mem floor >=10G.
cd "$(dirname "$0")/.." || exit 1
LOG=run/pa80-pair.log
M=run/pa78-branch-strong-bzImage
B=run/pa80-timer-strong-bzImage

stamp() { date '+%F %T'; }
l1() { awk '{printf "%d", $1}' /proc/loadavg; }
lane_q() {
  local n=0
  for pid in $(pgrep qemu-system 2>/dev/null); do
    if tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null | grep -q 'rux-agents/perf-audit'; then
      n=$((n+1))
    fi
  done
  echo "$n"
}
memfloor() { awk '/MemAvailable/{printf "%d", $2/1048576}' /proc/meminfo; }
wait_slot() {
  while [ "$(lane_q)" -ge 2 ] || [ "$(l1)" -gt 45 ] || [ "$(memfloor)" -lt 10 ]; do
    echo "$(stamp) $1 wait: lane_q=$(lane_q) load1=$(l1) mem=$(memfloor)G" >> "$LOG"
    sleep 60
  done
}

echo "=== pa80 timer A/B armed $(stamp) ===" >> "$LOG"
i=0
for arm in M B B M; do
  i=$((i+1)); tag="pa80pb${i}${arm}"
  k="$M"; [ "$arm" = "B" ] && k="$B"
  wait_slot "boot $i arm=$arm"
  echo "--- board arm=$arm $(stamp) load1=$(l1) mem=$(memfloor)G ---" >> "$LOG"
  python3 test/pbench_boot.py --bzimage "$k" --tag "$tag" \
      > "run/${tag}-driver.log" 2>&1
  echo "board $i arm=$arm rc=$? ($(stamp))" >> "$LOG"
  sleep 20
done
echo "=== pa80 done $(stamp) ===" >> "$LOG"
