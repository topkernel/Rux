#!/bin/bash
# pa74-pair.sh — leg 47: AF_UNIX read(2) direct user-drain A/B (the leg-46
# ledger 8.10.15 lever, WIP 2f66b0b8 completed).
#   M  = run/pa74-main-strong-bzImage   (0f806d8b strong, control)
#   B  = run/pa74-branch-strong-bzImage (main + merge + WIP drain)
# Interleaved M,B,B,M cancels host drift (pa68 method). Gate (IPC domain):
# P5 rows within +-20% of control or improved — especially p5c2/p5c3 and
# the p5e/p5f discriminators; the lever's target class is the p5c3/
# pin_recv64 regime (med 5-8 us) for the read(2) shape.
# Discipline: <=3 lane QEMUs total (incl. any pa73 underflow boots), own
# PIDs only, host mem floor >=10G, load1 ceiling 45 per boot.
cd "$(dirname "$0")/.." || exit 1
LOG=run/pa74-pair.log
M=run/pa74-main-strong-bzImage
B=run/pa74-branch-strong-bzImage

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

echo "=== pa74 AF_UNIX drain pair armed $(stamp) (M=$M B=$B) ===" >> "$LOG"
i=0
for arm in M B B M; do
  i=$((i+1)); tag="pa74pb${i}${arm}"
  k="$M"; [ "$arm" = "B" ] && k="$B"
  while [ "$(lane_q)" -ge 3 ] || [ "$(l1)" -gt 45 ] || [ "$(memfloor)" -lt 10 ]; do
    echo "$(stamp) boot $i arm=$arm wait: lane_q=$(lane_q) load1=$(l1) mem=$(memfloor)G" >> "$LOG"
    sleep 60
  done
  echo "--- boot $i arm=$arm tag=$tag $(stamp) load1=$(l1) lane_q=$(lane_q) ---" >> "$LOG"
  python3 test/pbench_boot.py --bzimage "$k" --tag "$tag" \
      --accel tcg,thread=multi --smp 2 --mem 2G --timeout 1200 \
      > "run/pa74-pb-${i}${arm}.log" 2>&1
  echo "boot $i arm=$arm rc=$? ($(stamp))" >> "$LOG"
  sleep 20
done
echo "=== pa74 pair complete $(stamp) ===" >> "$LOG"
