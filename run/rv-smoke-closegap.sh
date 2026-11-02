#!/bin/bash
# rv-smoke-closegap.sh — riscv64 regression boot for the closegap tree
# (gate: minimal rootfs, scripted serial cd/proc, rc=0, zero panics).
set -uo pipefail
WT="/home/william/rux-agents/perf-closegap"
KERNEL="$WT/target/riscv64gc-unknown-none-elf/debug/rux"
SERIAL="$WT/run/rv-smoke-closegap.log"
rm -f "$SERIAL"
( sleep 30; echo "cd /proc"; sleep 4; echo "ls"; sleep 4; echo "cd /"; sleep 2; echo "echo RVSMOKE-DONE"; sleep 2 ) | \
timeout --signal=TERM --kill-after=5 240s qemu-system-riscv64 \
    -M virt -accel tcg,thread=multi -cpu rv64 -m 1G -smp 2 \
    -display none -nographic -serial stdio -monitor none \
    -kernel "$KERNEL" \
    -drive file="$WT/test/rootfs.img",if=virtio,format=raw,file.locking=off,snapshot=on \
    -append "console=ttyS0" > "$SERIAL" 2>&1
rc=$?
echo "qemu rc=$rc"
echo "panics: $(grep -acE 'KERNPANIC|ALLOCTHROW|Kernel panic' "$SERIAL" || true)"
echo "done-mark: $(grep -ac 'RVSMOKE-DONE' "$SERIAL" || true)"
tail -4 "$SERIAL" | tr -d '\r' | cut -c1-140
