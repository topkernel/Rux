#!/bin/bash
# pa88-quiet-chain.sh — leg 51 Tier-1: quiet syscost census boot on the
# kswapd-leg tree (mm/ is a guarded domain: the deep pass must NEVER
# run on a standard boot — DEEP_PASS serial lines = 0, census in band,
# no reclaim-slowpath emergence). Kernel: run/pa88-dbg-bzImage (x86_64
# debug, the branch with the kswapd leg). Runs AFTER pa86 finishes
# (lane discipline, do not touch that chain) behind the same two-stage
# quiet gate. Parse via run/perf-gate.sh --skip-boot.
cd "$(dirname "$0")/.." || exit 1
LOG=run/pa88-quiet.log
K=run/pa88-dbg-bzImage

stamp() { date '+%F %T'; }
l1() { awk '{printf "%d", $1}' /proc/loadavg; }
foreign_q() {
  local n=0
  for pid in $(pgrep qemu-system 2>/dev/null); do
    tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null | grep -q 'rux-agents/perf-audit' || n=$((n+1))
  done
  echo "$n"
}
memfloor() { awk '/MemAvailable/{printf "%d", $2/1048576}' /proc/meminfo; }

quiet_wait() {
  local t1=0 t2=0
  while [ $((t1 + t2)) -lt 21600 ]; do
    if [ "$1" = stage1 ]; then
      if [ "$(l1)" -le 9 ] && [ "$(foreign_q)" -le 8 ] && [ "$(memfloor)" -ge 10 ]; then return 0; fi
    else
      if [ "$(l1)" -le 13 ] && [ "$(foreign_q)" -le 10 ] && [ "$(memfloor)" -ge 10 ]; then return 0; fi
    fi
    sleep 60
    if [ "$1" = stage1 ]; then t1=$((t1+60)); else t2=$((t2+60)); fi
    if [ "$1" = stage1 ] && [ $t1 -ge 7200 ]; then echo "$(stamp) stage1 window never opened, relaxing" >> "$LOG"; return 0; fi
  done
  return 1
}

echo "=== pa88 quiet chain armed $(stamp) (kernel $K) ===" >> "$LOG"
[ -f "$K" ] || { echo "$(stamp) kernel missing — abort" >> "$LOG"; exit 1; }

# wait for pa86 to finish first (it owns the quiet-window slot)
while ! grep -q "=== pa86 done" run/pa86-quiet.log 2>/dev/null; do
  sleep 120
done
echo "$(stamp) pa86 finished — proceeding to quiet gate" >> "$LOG"

quiet_wait stage1 || { echo "$(stamp) no window in 6 h — skipped (logged debt)" >> "$LOG"; exit 0; }
quiet_wait stage2 || true
echo "$(stamp) window: load1=$(l1) foreign=$(foreign_q) mem=$(memfloor)G — booting pa88dbg 780s" >> "$LOG"

SMP=2 ACCEL=kvm APPEND_EXTRA="syscost=1 dfx=wwcensus" bash run/pa-boot.sh pa88dbg "$K" 780 \
    > run/pa88-boot.log 2>&1
echo "$(stamp) census boot rc=$? deep_lines=$(grep -c OOM-DEEP-PASS run/pa-pa88dbg-serial.log 2>/dev/null) term=$(grep -c 'TERM-DESCHED engaged' run/pa-pa88dbg-serial.log 2>/dev/null) refused=$(grep -c 'WAKE-REFUSED' run/pa-pa88dbg-serial.log 2>/dev/null)" >> "$LOG"

bash run/perf-gate.sh --skip-boot --log run/pa-pa88dbg-stamped.log --tag pa88 \
    > run/pa88-gate-parse.txt 2>&1
echo "$(stamp) gate parse rc=\$? (run/pa88-gate-parse.txt)" >> "$LOG"
echo "=== pa88 done $(stamp) ===" >> "$LOG"
