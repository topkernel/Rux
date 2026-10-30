#!/bin/bash
# fm-rv-smoke.sh: riscv64 minimal-rootfs console smoke (session-22 shape,
# 150 s to the mrsh prompt) for the filemap_fault kernel.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SERIAL="$ROOT/run/gb-smoke-fix1-serial.log"
rm -f "$SERIAL"
timeout --signal=TERM --kill-after=5 150s qemu-system-riscv64 \
    -M virt -accel tcg,thread=multi -cpu rv64 -m 1G -smp 2 \
    -display none -serial file:"$SERIAL" \
    -kernel "$ROOT/target/riscv64gc-unknown-none-elf/debug/rux" \
    -drive file="$ROOT/test/rootfs.img",if=virtio,format=raw,file.locking=off,snapshot=on \
    -append "console=ttyS0" || true
echo "panics: $(grep -cE 'KERNPANIC|ALLOCTHROW' "$SERIAL" || true)"
grep -cE "/bin/sh|mrsh|# " "$SERIAL" || true
