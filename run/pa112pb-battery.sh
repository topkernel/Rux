#!/bin/bash
# pa112pb-battery.sh — Tier-2 fs-family re-bank on main@82059759
# (debug bzImage run/pa110-main-dbg-bzImage, md5 c800ffe3710b07b8).
# Two quiet pbench boards (defaults = pa-series machine class: q35 TCG
# thread=multi, smp2, mem 2G, usbless minimal rootfs — independent of
# the OH image lineage). Whole table read FROM THE SERIAL LOG (the
# pbench_boot stdout view drops rows — pa104 evidence). Debug class =>
# compared against the pb1 debug anchor column.
# Discipline: sequential boards, load1<=45, own PIDs only.
cd /home/william/rux-agents/perf-audit || exit 1
LOG=run/pa112pb-battery.log
K=/home/william/rux-agents/perf-audit/run/pa110-main-dbg-bzImage

stamp() { date '+%F %T'; }
l1() { awk '{printf "%d", $1}' /proc/loadavg; }
memfloor() { awk '/MemAvailable/{printf "%d", $2/1048576}' /proc/meminfo; }
wait_slot() {
  while [ "$(l1)" -gt 45 ] || [ "$(memfloor)" -lt 10 ]; do
    echo "$(stamp) $1 wait: load1=$(l1) mem=$(memfloor)G" >> "$LOG"
    sleep 60
  done
}
table_read() {
  local ser="$1" tag="$2"
  {
    echo "--- TABLE $tag ($(stamp)) ---"
    grep "PBENCH PREP\|PBENCH p6_\|PBENCH DONE\|PBENCH p1_\|PBENCH p2_" "$ser" 2>/dev/null
    echo "panics=$(grep -c 'KERNPANIC' "$ser" 2>/dev/null)"
  } >> "$LOG"
}

echo "=== pa112pb battery armed $(stamp) (K=$K md5=$(md5sum "$K" | cut -c1-12)) ===" >> "$LOG"

for rep in 1 2; do
  tag="pa112pb${rep}"
  wait_slot "pbench $rep"
  echo "--- pbench boot $rep tag=$tag $(stamp) load1=$(l1) mem=$(memfloor)G ---" >> "$LOG"
  python3 test/pbench_boot.py --bzimage "$K" --tag "$tag" \
      --timeout 1200 > "run/pa112pb-${rep}.log" 2>&1
  echo "pbench boot $rep rc=$? ($(stamp))" >> "$LOG"
  table_read "work/pbench-${tag}-serial.log" "$tag"
  sleep 20
done
echo "=== pa112pb battery complete $(stamp) ===" >> "$LOG"
