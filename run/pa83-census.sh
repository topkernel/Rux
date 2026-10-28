#!/bin/bash
# pa83-census.sh — leg 49 debt: Tier-1 census boot on the COMBINED leg-49
# tip (lazy revoke d4addcfc + R37 timer 7ef5a170 + vmscan deep pass
# 39a515a5; x86_64 debug bzImage run/pa83-tip-bzImage). Two-stage quiet
# gate (pa57 contract): first 2 h load1<=9 & lane-foreign qemus<=8, then
# load1<=13 & <=10; 6 h deadline; skip logged if no window. On window:
# 780 s KVM OH boot via pa-boot.sh (syscost=1 fsw=1), parse with
# perf-gate.sh --skip-boot, and assert the vmscan deep-pass counters'
# serial absence (OOM-DEEP-PASS must not appear on a standard boot —
# the no-reclaim-slowpath-emergence discipline for the new path).
cd "$(dirname "$0")/.." || exit 1
LOG=run/pa83-census.log
K=run/pa83-tip-bzImage

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

quiet_wait() { # $1 stage-1 secs, $2 stage-2 secs
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

echo "=== pa83 census chain armed $(stamp) (kernel $K) ===" >> "$LOG"
[ -f "$K" ] || { echo "$(stamp) kernel missing — abort" >> "$LOG"; exit 1; }

quiet_wait stage1 || { echo "$(stamp) no window in 6 h — skipped (logged debt)" >> "$LOG"; exit 0; }
quiet_wait stage2 || true
echo "$(stamp) window: load1=$(l1) foreign=$(foreign_q) mem=$(memfloor)G — booting pa83tip 780s" >> "$LOG"

SMP=2 ACCEL=kvm APPEND_EXTRA="syscost=1 fsw=1" bash run/pa-boot.sh pa83tip "$K" 780 \
    > run/pa83-boot.log 2>&1
echo "$(stamp) boot rc=$? deep_lines=$(grep -c OOM-DEEP-PASS run/pa-pa83tip-serial.log 2>/dev/null)" >> "$LOG"

python3 test/perf-gate.sh --skip-boot > run/pa83-gate-parse.txt 2>&1
echo "$(stamp) gate parse rc=$? (run/pa83-gate-parse.txt)" >> "$LOG"
echo "=== pa83 done $(stamp) ===" >> "$LOG"
