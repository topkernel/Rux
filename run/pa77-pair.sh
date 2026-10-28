#!/bin/bash
# pa77-pair.sh — leg 48: re-gate the AF_UNIX read(2) direct user-drain
# (d30f40de, code 2f66b0b8) against the NEW main tip after merging
# origin/main 584c1105 (console cond_resched + binder set land between
# 0f806d8b and 584c1105 — serial-burst scheduling changed, so the leg-47
# A/B vs 0f806d8b must be re-anchored before ready-for-merge).
#   M = run/pa77-main-strong-bzImage   (584c1105 strong, control)
#   B = run/pa77-branch-strong-bzImage (9baf390a = 584c1105 + drain)
# Order: pin M, pin B (deterministic samecpu windows, the leg-46/47
# discriminators), then board M,B,B,M interleaved pairs (pa68/pa74
# method). Gate (IPC domain): P5 rows within +-20% of control or
# improved; pin_r64/pin_r4k should hold the leg-47 -15% class;
# pin_recv64/spread untouched-class in band.
# Discipline: <=3 lane QEMUs, own PIDs only, mem floor >=10G, load1
# ceiling 45 per boot.
cd "$(dirname "$0")/.." || exit 1
LOG=run/pa77-pair.log
M=run/pa77-main-strong-bzImage
B=run/pa77-branch-strong-bzImage

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
  while [ "$(lane_q)" -ge 3 ] || [ "$(l1)" -gt 45 ] || [ "$(memfloor)" -lt 10 ]; do
    echo "$(stamp) $1 wait: lane_q=$(lane_q) load1=$(l1) mem=$(memfloor)G" >> "$LOG"
    sleep 60
  done
}

echo "=== pa77 drain re-gate armed $(stamp) (M=$M B=$B) ===" >> "$LOG"

# --- deterministic samecpu pin windows, adjacent M then B ---
for arm in M B; do
  k="$M"; [ "$arm" = "B" ] && k="$B"
  wait_slot "pin $arm"
  echo "--- pin arm=$arm $(stamp) load1=$(l1) lane_q=$(lane_q) ---" >> "$LOG"
  python3 test/pbench_boot.py --bzimage "$k" --tag "pa77pin${arm}" \
      --accel tcg,thread=multi --smp 2 --mem 2G --timeout 1200 --long-p5 pin \
      > "run/pa77-pin${arm}.log" 2>&1
  echo "pin arm=$arm rc=$? ($(stamp))" >> "$LOG"
  sleep 20
done

# --- interleaved board pairs M,B,B,M ---
i=0
for arm in M B B M; do
  i=$((i+1)); tag="pa77pb${i}${arm}"
  k="$M"; [ "$arm" = "B" ] && k="$B"
  wait_slot "boot $i arm=$arm"
  echo "--- boot $i arm=$arm tag=$tag $(stamp) load1=$(l1) lane_q=$(lane_q) ---" >> "$LOG"
  python3 test/pbench_boot.py --bzimage "$k" --tag "$tag" \
      --accel tcg,thread=multi --smp 2 --mem 2G --timeout 1200 \
      > "run/pa77-pb-${i}${arm}.log" 2>&1
  echo "boot $i arm=$arm rc=$? ($(stamp))" >> "$LOG"
  sleep 20
done
echo "=== pa77 pair complete $(stamp) ===" >> "$LOG"
