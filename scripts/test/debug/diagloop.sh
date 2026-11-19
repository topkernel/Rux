#!/bin/bash
# R56-heal diagnosis: loop repro-full (non-truncating panic capture) until a
# KERNPANIC with full dump lands, up to N attempts. One QEMU at a time.
set -u
BASE=/home/william/rux-agents/r56-heal/diag
K=/home/william/rux-agents/r56-heal/target/riscv64gc-unknown-none-elf/debug/rux
N=${1:-6}
for i in $(seq 4 $((3+N))); do
  out="$BASE/d$i"
  rm -rf "$out"; mkdir -p "$out"
  REPRO_KERNEL=$K python3 /home/william/rux-agents/r56-heal/run/repro-full.py "$out" 480 > "$out/driver.log" 2>&1
  kcount=$(strings "$out/serial.log" 2>/dev/null | grep -cE "KERNPANIC")
  epc=$(strings "$out/serial.log" 2>/dev/null | grep -oE "epc=0x[0-9a-f]+" | head -1)
  echo "diag d$i: kernpanics=$kcount $epc $(date +%H:%M:%S)"
  if [ "$kcount" -gt 0 ] && [ -n "$epc" ]; then
    echo "CAPTURED: $out/serial.log"
    break
  fi
  rm -f "$out/fire.img"
done
