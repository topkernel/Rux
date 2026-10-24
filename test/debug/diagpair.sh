#!/bin/bash
# Two concurrent repro-full lanes to induce the MTTCG contention crash and
# capture a full panic dump. Stops both when one captures epc.
set -u
BASE=/home/william/rux-agents/r56-heal/diag
K=/home/william/rux-agents/r56-heal/target/riscv64gc-unknown-none-elf/debug/rux
round=${1:-1}
lane() {
  local L=$1
  for r in $(seq $round 8); do
    local out="$BASE/p${L}_$r"
    rm -rf "$out"; mkdir -p "$out"
    REPRO_KERNEL=$K python3 /home/william/rux-agents/r56-heal/run/repro-full.py "$out" 480 > "$out/driver.log" 2>&1
    local k e
    k=$(strings "$out/serial.log" 2>/dev/null | grep -cE "KERNPANIC")
    e=$(strings "$out/serial.log" 2>/dev/null | grep -oE "epc=0x[0-9a-f]+" | head -1)
    echo "lane$L r$r: panics=$k $e $(date +%H:%M:%S)"
    if [ "$k" -gt 0 ] && [ -n "$e" ]; then
      echo "lane$L CAPTURED $out/serial.log" >> "$BASE/CAPTURED"
      break
    fi
    rm -f "$out/fire.img"
  done
}
lane a & lane b & wait
