#!/bin/bash
# pa86-quiet-chain.sh — leg 50: new-main re-anchor chain on 2a960538+
# kernel content (branch a8a5afac). Two jobs behind one quiet gate:
#   (a) Tier-1 syscost census boot, DEBUG kernel run/pa86-dbg-bzImage,
#       APPEND_EXTRA="syscost=1 dfx=wwcensus" — the census token was
#       MISSING on pa83 (its APPEND_EXTRA override dropped it; the
#       0/0 census rows were an instrument gap, not a regression) —
#       this boot re-reads TERM/REFUSED on the merged main, closes the
#       pa83 debt formally, and carries the OOM-DEEP-PASS absence
#       assertion (vmscan deep pass never runs on a standard boot).
#   (b) pbench quiet anchor board, STRONG kernel
#       run/pa84-main-strong-bzImage, 2 boots (med-of-2): the new
#       standing P6 classes after scoped-publication + arm-2 + the
#       zero-serve device-truthed completions (the IO rows moved twice
#       on 2026-10-28; this banks the settled classes).
# Gate: pa85 chain finished first (1 lane board at a time), then the
# pa57 two-stage contract (stage1 2h load1<=9 & foreign<=8, then
# relaxed load1<=13 & <=10), 8 h deadline, skip logged as debt.
# Discipline: 1 lane QEMU at a time, own PIDs only, mem floor >=10G.
cd "$(dirname "$0")/.." || exit 1
LOG=run/pa86-quiet.log
DBG=run/pa86-dbg-bzImage
STRONG=run/pa84-main-strong-bzImage

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

echo "=== pa86 quiet chain armed $(stamp) (dbg=$DBG strong=$STRONG) ===" >> "$LOG"
[ -f "$DBG" ] && [ -f "$STRONG" ] || { echo "$(stamp) kernel missing — abort" >> "$LOG"; exit 1; }

# --- wait for the pa85 chain to finish (lane discipline) ---
while ! grep -q "=== pa85 done" run/pa85-pair.log 2>/dev/null; do
  sleep 60
  grep -q "pa85.*abort" run/pa85-pair.log 2>/dev/null && break
done
echo "$(stamp) pa85 finished — proceeding to quiet gate" >> "$LOG"

quiet_wait stage1 || { echo "$(stamp) no window in 8 h — skipped (logged debt)" >> "$LOG"; exit 0; }
quiet_wait stage2 || true
echo "$(stamp) window: load1=$(l1) foreign=$(foreign_q) mem=$(memfloor)G — census boot first" >> "$LOG"

# --- (a) Tier-1 census boot, census token EXPLICIT this time ---
SMP=2 ACCEL=kvm APPEND_EXTRA="syscost=1 dfx=wwcensus" bash run/pa-boot.sh pa86dbg "$DBG" 780 \
    > run/pa86-boot.log 2>&1
echo "$(stamp) census boot rc=$? deep_lines=$(grep -c OOM-DEEP-PASS run/pa-pa86dbg-serial.log 2>/dev/null) term=$(grep -c 'TERM-DESCHED engaged' run/pa-pa86dbg-serial.log 2>/dev/null) refused=$(grep -c 'WAKE-REFUSED' run/pa-pa86dbg-serial.log 2>/dev/null)" >> "$LOG"

bash run/perf-gate.sh --skip-boot --log run/pa-pa86dbg-stamped.log --tag pa86 \
    > run/pa86-gate-parse.txt 2>&1
echo "$(stamp) gate parse rc=$? (run/pa86-gate-parse.txt)" >> "$LOG"

# --- (b) pbench quiet anchor board on the strong kernel ---
sleep 30
for i in 1 2; do
  tag="pa86q${i}"
  while [ "$(l1)" -gt 45 ] || [ "$(memfloor)" -lt 10 ]; do sleep 60; done
  echo "--- board $i $(stamp) load1=$(l1) mem=$(memfloor)G ---" >> "$LOG"
  python3 test/pbench_boot.py --bzimage "$STRONG" --tag "$tag" \
      > "run/${tag}-driver.log" 2>&1
  echo "board $i rc=$? ($(stamp))" >> "$LOG"
  sleep 20
done

echo "=== pa86 done $(stamp) ===" >> "$LOG"
