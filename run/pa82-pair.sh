#!/bin/bash
# pa82-pair.sh — leg 49B: vmscan deep pre-OOM pass A/B (mm domain,
# Tier-2 rows; the functional evidence is the oomchurn fake A/B in
# run/pa81-{m,b}-fake*.log).
#   M = run/pa82-m-strong-bzImage (7ef5a170 = lazy revoke + R37 timer)
#   B = run/pa82-b-strong-bzImage (M + vmscan deep pass, 39a515a5)
# The branch delta vs M is exactly the mm change (plus the userspace
# oomchurn driver, which does not run here). Expected row-neutral: the
# deep pass executes only on the doomed slowpath after 8 failed rounds,
# which pbench loads never reach. Gate (Tier-2): P1-P7 medians within
# +-20% of M; p2_ctxsw judged against the strong anchors (quiet 1.37,
# loaded ceiling ~2.0). Additionally assert no OOM-DEEP-PASS line in
# any serial (the counter discipline: standard boots never drain).
# Discipline: 1 lane QEMU at a time, own PIDs only, mem floor >=10G.
cd "$(dirname "$0")/.." || exit 1
LOG=run/pa82-pair.log
M=run/pa82-m-strong-bzImage
B=run/pa82-b-strong-bzImage

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

echo "=== pa82 vmscan A/B armed $(stamp) ===" >> "$LOG"
i=0
for arm in M B B M; do
  i=$((i+1)); tag="pa82pb${i}${arm}"
  k="$M"; [ "$arm" = "B" ] && k="$B"
  wait_slot "boot $i arm=$arm"
  echo "--- board arm=$arm $(stamp) load1=$(l1) mem=$(memfloor)G ---" >> "$LOG"
  python3 test/pbench_boot.py --bzimage "$k" --tag "$tag" \
      > "run/${tag}-driver.log" 2>&1
  echo "board $i arm=$arm rc=$? ($(stamp)) deep_lines=$(grep -c OOM-DEEP-PASS work/pbench-${tag}-serial.log 2>/dev/null)" >> "$LOG"
  sleep 20
done
echo "=== pa82 done $(stamp) ===" >> "$LOG"
