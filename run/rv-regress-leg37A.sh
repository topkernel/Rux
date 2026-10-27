#!/bin/bash
# rv-regress-leg37A.sh — riscv64 scripted regression boot for the
# wake-placement-reservation + main-584c1105 merged tip (smp2 1G, dfx=wwcensus for the hint
# counters; serial-clean expectations unchanged).
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SERIAL="$ROOT/run/rv-regress-leg37A-serial.log"
rm -f "$SERIAL"
timeout --signal=TERM --kill-after=5 240s qemu-system-riscv64 \
    -M virt -accel tcg,thread=multi -cpu rv64 -m 1G -smp 2 \
    -display none -serial file:"$SERIAL" \
    -kernel "$ROOT/target/riscv64gc-unknown-none-elf/debug/rux" \
    -drive file="$ROOT/test/rootfs.img",if=virtio,format=raw,file.locking=off,snapshot=on \
    -append "console=ttyS0 dfx=wwcensus" || true
echo "== verdicts =="
echo "panics: $(grep -cE 'KERNPANIC|ALLOCTHROW' "$SERIAL" || true)"
echo "PREEMPT-UNDERFLOW: $(grep -c 'PREEMPT-UNDERFLOW' "$SERIAL" || true)"
echo "WAKE-STRANDED: $(grep -c 'WAKE-STRANDED' "$SERIAL" || true)"
echo "shell prompt: $(grep -cE '/bin/sh|mrsh|# ' "$SERIAL" || true)"
echo "WW lines: $(grep -c 'WW: wakes=' "$SERIAL" || true)"
echo "hint tail: $(grep 'WW: wakes=' "$SERIAL" | tail -1)"
