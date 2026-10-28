#!/bin/bash
# pa78-pair.sh — leg 49: F4 arm-2 lazy-revoke A/B (fs domain).
#   M = run/pa78-main-strong-bzImage   (pure main f15b9660 strong, control)
#   B = run/pa78-branch-strong-bzImage (9a80066b docs + d4addcfc lazy revoke)
# Gate (fs domain): p6_mkdrmdir and p6_unlink are the target rows
# (prediction: 52-58ms -> single-digit ms; ~16ms -> low ms);
# p6_write_fsync / p6_append / p6_stat / p6_append and the P5 rows are
# the in-band guards (+-20% or improved). Interleaved M,B,B,M board
# pairs, then one instrumented pair (fsw=1) for the jcommit_slow
# mechanism probe (expect: ~= unlink+rmdir count on M, a handful on B).
# Discipline: 1 lane QEMU at a time (sequential), own PIDs only, mem
# floor >=10G, load1 ceiling 45 per boot.
cd "$(dirname "$0")/.." || exit 1
LOG=run/pa78-pair.log
M=run/pa78-main-strong-bzImage
B=run/pa78-branch-strong-bzImage

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

echo "=== pa78 lazy-revoke A/B armed $(stamp) (M=$M B=$B) ===" >> "$LOG"

# --- interleaved board pairs M,B,B,M (plain, gate numbers) ---
i=0
for arm in M B B M; do
  i=$((i+1)); tag="pa78pb${i}${arm}"
  k="$M"; [ "$arm" = "B" ] && k="$B"
  wait_slot "boot $i arm=$arm"
  echo "--- board arm=$arm $(stamp) load1=$(l1) mem=$(memfloor)G ---" >> "$LOG"
  python3 test/pbench_boot.py --bzimage "$k" --tag "$tag" \
      > "run/${tag}-driver.log" 2>&1
  echo "board $i arm=$arm rc=$? ($(stamp))" >> "$LOG"
  sleep 20
done

# --- instrumented pair for the fsw mechanism probe ---
for arm in M B; do
  tag="pa78fsw${arm}"
  k="$M"; [ "$arm" = "B" ] && k="$B"
  wait_slot "fsw arm=$arm"
  echo "--- fsw arm=$arm $(stamp) load1=$(l1) ---" >> "$LOG"
  python3 test/pbench_boot.py --bzimage "$k" --tag "$tag" \
      --append-extra "fsw=1" \
      > "run/${tag}-driver.log" 2>&1
  echo "fsw arm=$arm rc=$? ($(stamp))" >> "$LOG"
  sleep 20
done

echo "=== pa78 done $(stamp) ===" >> "$LOG"
