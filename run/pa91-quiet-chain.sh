#!/bin/bash
# pa91-quiet-chain.sh — leg 52 measurement debt on the lever-A+B tree
# (commits cceb808f + fd5034e3 + ac53dd6e):
#   (a) Tier-1 syscost census + REST RECOUNT boot, DEBUG kernel
#       run/pa91-dbg-bzImage: the file.rs get_file REST site was
#       132-144M calls / 49-51s per boot at the 342-383ns class (pa86
#       board). Lever A should cut the VOLUME (one lookup per syscall,
#       not 2-6); lever B cuts the PER-CALL cost (no entry-lock TTAS, no
#       preempt pair). The recount reads the new site row (new line
#       number — the x86 arch's save_and_disable_irq is #[track_caller],
#       so attribution follows get_file) plus the standard Tier-1 rows
#       (census in band, ticks, volume row — doubles as the pa86/pa88
#       volume isolation rerun per the rerun-once rule).
#   (b) pbench med-of-2 boards, STRONG kernel run/pa90-leverB-strong-
#       bzImage: the item-1 pbench gate (fd-heavy rows p6_openclose /
#       p4_fork_exec / dup-close paths within band) + the p5b dgram
#       ADJACENT-PAIR measurement (item 2 regime pin: pa90func loaded
#       board read 4.5/4.0us where every pa85/pa86 board read 39-43us —
#       the quiet pair decides lever-vs-regime-flip) + the standing
#       classes for the baseline refresh.
# Gate: two-stage (stage1 2h load1<=9 & foreign<=8, relax load1<=13 &
# foreign<=10), 8h deadline, skip logged as debt. Sequential boots, own
# PIDs only, mem floor >=10G.
cd "$(dirname "$0")/.." || exit 1
LOG=run/pa91-quiet.log
DBG=run/pa91-dbg-bzImage
STRONG=run/pa90-leverB-strong-bzImage

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
  while [ $((t1 + t2)) -lt 28800 ]; do
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

echo "=== pa91 quiet chain armed $(stamp) (dbg=$DBG strong=$STRONG) ===" >> "$LOG"
[ -f "$DBG" ] && [ -f "$STRONG" ] || { echo "$(stamp) kernel missing — abort" >> "$LOG"; exit 1; }

quiet_wait stage1 || { echo "$(stamp) no window in 8 h — skipped (logged debt)" >> "$LOG"; exit 0; }
quiet_wait stage2 || true
echo "$(stamp) window: load1=$(l1) foreign=$(foreign_q) mem=$(memfloor)G — census+REST boot first" >> "$LOG"

# --- (a) Tier-1 census + REST recount ---
SMP=2 ACCEL=kvm APPEND_EXTRA="syscost=1 dfx=wwcensus" bash run/pa-boot.sh pa91dbg "$DBG" 780 \
    > run/pa91-boot.log 2>&1
echo "$(stamp) census boot rc=$? deep_lines=$(grep -c OOM-DEEP-PASS run/pa-pa91dbg-serial.log 2>/dev/null) term=$(grep -c 'TERM-DESCHED engaged' run/pa-pa91dbg-serial.log 2>/dev/null) refused=$(grep -c 'WAKE-REFUSED' run/pa-pa91dbg-serial.log 2>/dev/null)" >> "$LOG"

bash run/perf-gate.sh --skip-boot --log run/pa-pa91dbg-stamped.log --tag pa91 \
    > run/pa91-gate-parse.txt 2>&1
echo "$(stamp) gate parse (run/pa91-gate-parse.txt; REST recount = the file.rs get_file row in run/perf-gate-pa91-parse.txt)" >> "$LOG"

# --- (b) pbench med-of-2 on the lever tree ---
sleep 30
for i in 1 2; do
  tag="pa91q${i}"
  while [ "$(l1)" -gt 45 ] || [ "$(memfloor)" -lt 10 ]; do sleep 60; done
  echo "--- board $i $(stamp) load1=$(l1) mem=$(memfloor)G ---" >> "$LOG"
  python3 test/pbench_boot.py --bzimage "$STRONG" --tag "$tag" \
      > "run/${tag}-driver.log" 2>&1
  echo "board $i rc=$? ($(stamp))" >> "$LOG"
  sleep 20
done

echo "=== pa91 done $(stamp) ===" >> "$LOG"
