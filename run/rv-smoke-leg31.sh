#!/bin/bash
# rv-smoke-leg31.sh: riscv64 regression smoke for the leg-31 tree
# (probe commit f46c6ad9 + interleaved leg-29A commits).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SERIAL="$ROOT/run/rv-smoke-leg31-serial.log"
rm -f "$SERIAL"
timeout --signal=TERM --kill-after=5 150s qemu-system-riscv64 \
    -M virt -accel tcg,thread=multi -cpu rv64 -m 1G -smp 2 \
    -display none -serial file:"$SERIAL" \
    -kernel "$ROOT/target/riscv64gc-unknown-none-elf/debug/rux" \
    -drive file="$ROOT/test/rootfs.img",if=virtio,format=raw,file.locking=off,snapshot=on \
    -append "console=ttyS0" || true
echo "panics: $(grep -cE 'KERNPANIC|ALLOCTHROW' "$SERIAL" || true)"
echo "shell prompt: $(grep -cE '/bin/sh|mrsh' "$SERIAL" || true)"
echo "proc mount: $(grep -c 'cd /proc\|/proc' "$SERIAL" || true)"
tail -3 "$SERIAL" | cut -c1-140
