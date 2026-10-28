#!/bin/bash
# pa85-f4nj-pair.sh — leg 50: F4 arm-1, the J-vs-NJ p6 comparison,
# UNBLOCKED by main's zero-serve fix (1e27b095, in 2a960538: the
# no-journal root-lookup failure was root-caused to a buffer-cache
# entry published BH_Uptodate before its DMA landed). ONE kernel,
# run/pa84-main-strong-bzImage (release-strong, a8a5afac = 2a960538
# kernel content + README-only delta). Two image arms from
# pbench_boot.py: J = default mkfs ^metadata_csum (journal engages),
# NJ = --nojournal (^metadata_csum,^has_journal — the OH/GNOME
# userdata image class; route-1's target).
# Purpose: bank the journal-vs-nojournal gap on the p6 rows as the
# route-1 expected-effect anchor (the fs lane's lever), plus a fsw=1
# pair for the sync-write volume mechanism probe.
# Discipline: 1 lane QEMU at a time (sequential), own PIDs only, mem
# floor >=10G, load1 ceiling 45 per boot.
cd "$(dirname "$0")/.." || exit 1
LOG=run/pa85-pair.log
K=run/pa84-main-strong-bzImage

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

echo "=== pa85 F4 arm-1 J-vs-NJ armed $(stamp) (kernel $K) ===" >> "$LOG"
[ -f "$K" ] || { echo "$(stamp) kernel missing — abort" >> "$LOG"; exit 1; }

# --- interleaved board pairs J,NJ,NJ,J (plain, gate numbers) ---
i=0
for arm in J NJ NJ J; do
  i=$((i+1)); tag="pa85pb${i}${arm}"
  wait_slot "boot $i arm=$arm"
  echo "--- board arm=$arm $(stamp) load1=$(l1) mem=$(memfloor)G ---" >> "$LOG"
  if [ "$arm" = "NJ" ]; then
    python3 test/pbench_boot.py --bzimage "$K" --tag "$tag" --nojournal \
        > "run/${tag}-driver.log" 2>&1
  else
    python3 test/pbench_boot.py --bzimage "$K" --tag "$tag" \
        > "run/${tag}-driver.log" 2>&1
  fi
  echo "board $i arm=$arm rc=$? ($(stamp))" >> "$LOG"
  sleep 20
done

# --- instrumented pair for the fsw sync-write volume probe ---
for arm in J NJ; do
  tag="pa85fsw${arm}"
  wait_slot "fsw arm=$arm"
  echo "--- fsw arm=$arm $(stamp) load1=$(l1) ---" >> "$LOG"
  if [ "$arm" = "NJ" ]; then
    python3 test/pbench_boot.py --bzimage "$K" --tag "$tag" --nojournal \
        --append-extra "fsw=1" \
        > "run/${tag}-driver.log" 2>&1
  else
    python3 test/pbench_boot.py --bzimage "$K" --tag "$tag" \
        --append-extra "fsw=1" \
        > "run/${tag}-driver.log" 2>&1
  fi
  echo "fsw arm=$arm rc=$? ($(stamp))" >> "$LOG"
  sleep 20
done

echo "=== pa85 done $(stamp) ===" >> "$LOG"
