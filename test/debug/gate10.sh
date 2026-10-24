#!/bin/bash
# R56-heal gate 1: gnome fresh-copy boot x10, MTTCG smp4, zero panic.
# Discipline: one QEMU at a time, cold copy per run, file.locking=off
# (repro.py), kill only its own pid (repro.py does), clean fire.img after.
set -u
KERNEL=/home/william/rux-agents/r56-heal/target/riscv64gc-unknown-none-elf/debug/rux
REPRO=/home/william/rux-agents/gnome-sigill/repro.py
BASE=/home/william/rux-agents/r56-heal/gate-runs
mkdir -p "$BASE"
PASS=0; FAIL=0; INVALID=0
START=${GATE_START:-1}
for i in $(seq "$START" 10); do
  out="$BASE/g$i"
  rm -rf "$out"; mkdir -p "$out"
  REPRO_KERNEL="$KERNEL" python3 "$REPRO" "$out" 420 > "$out/driver.log" 2>&1
  rc=$?
  ser="$out/serial.log"
  if [ ! -s "$ser" ]; then
    # QEMU may have failed to start under host memory pressure: retry once.
    sleep 60
    REPRO_KERNEL="$KERNEL" python3 "$REPRO" "$out" 420 > "$out/driver-retry.log" 2>&1
    rc=$?
  fi
  panic=$(strings "$ser" 2>/dev/null | grep -cE "KERNPANIC|Kernel panic|SIGILL-FORENSIC|trap: user SIGILL")
  xorg=$(strings "$ser" 2>/dev/null | grep -c "BG /usr/bin/Xorg")
  if [ "$panic" -gt 0 ]; then
    verdict=FAIL
    FAIL=$((FAIL+1))
  elif [ "$xorg" -eq 0 ]; then
    verdict=INVALID
    INVALID=$((INVALID+1))
  else
    verdict=PASS
    PASS=$((PASS+1))
  fi
  echo "run $i: $verdict (rc=$rc panic=$panic xorg_marker=$xorg) $(date +%H:%M:%S)" >> "$BASE/summary.txt"
  echo "gate1 run $i/10: $verdict panic=$panic xorg=$xorg"
  rm -f "$out/fire.img"   # reclaim 2.5G sparse per run
done
echo "gate1 DONE: PASS=$PASS FAIL=$FAIL INVALID=$INVALID" | tee -a "$BASE/summary.txt"
