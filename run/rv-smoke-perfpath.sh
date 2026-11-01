#!/bin/bash
# rv-smoke-perfpath.sh — riscv64 regression boot for the perf-path tree
# (gate: minimal rootfs, scripted serial cd/proc, rc=0, zero panics).
set -uo pipefail
WT="/home/william/rux-agents/perf-path"
KERNEL="$WT/target/riscv64gc-unknown-none-elf/debug/rux"
SERIAL="$WT/run/rv-smoke-perfpath.log"
rm -f "$SERIAL"
( sleep 25; echo "cd /proc"; sleep 3; echo "ls"; sleep 3; echo "cd /"; sleep 2; echo "echo RVSMOKE-DONE"; sleep 2 ) | \
timeout --signal=TERM --kill-after=5 180s qemu-system-riscv64 \
    -M virt -accel tcg,thread=multi -cpu rv64 -m 1G -smp 2 \
    -display none -nographic -serial stdio -monitor none \
    -kernel "$KERNEL" \
    -drive file="$WT/test/rootfs.img",if=virtio,format=raw,file.locking=off,snapshot=on \
    -append "console=ttyS0" > "$SERIAL" 2>&1 || true
echo "panics: $(grep -acE 'KERNPANIC|ALLOCTHROW|panic' "$SERIAL" || true)"
echo "shell: $(grep -acE 'mrsh|/bin/sh|# |\$ ' "$SERIAL" || true)"
echo "proc-cd: $(grep -ac 'proc' "$SERIAL" || true)"
echo "done-mark: $(grep -ac 'RVSMOKE-DONE' "$SERIAL" || true)"
tail -5 "$SERIAL" | cut -c1-140
