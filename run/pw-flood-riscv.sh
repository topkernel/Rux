#!/bin/bash
# pw-flood-riscv.sh — printk flood soak (riscv64, SMP2, TCG MTTCG).
#
# Boots the minimal rootfs straight into /test/printk_flood as init:
# 2 mremap flooders (each iteration = one MREMAP-CFI kernel printk line,
# the OH b140 flood family) + 1 /dev/console writer, for 600s. dfx=periodic
# adds periodic task dumps from the timer softirq so printk also runs in
# interrupt context while the flood holds the console busy — the b140
# wedge shape. The driver allows 780s wall (boot + flood + margin), then
# TERMs QEMU.
#
# Usage: pw-flood-riscv.sh [seconds]
set -u
ROOT=/home/william/rux-agents/printk-wedge
cd "$ROOT"
LOG="$ROOT/run/pw-flood-riscv-serial.log"
rm -f "$LOG"

timeout --signal=TERM --kill-after=15 "${1:-780}s" qemu-system-riscv64 \
    -M virt -accel tcg,thread=multi \
    -cpu rv64,zbb=true,zba=true,zbs=true \
    -m 1G -smp 2 \
    -nographic \
    -serial file:"$LOG" \
    -monitor none \
    -drive file=test/rootfs.img,if=none,id=rootfs,format=raw \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -kernel target/riscv64gc-unknown-none-elf/debug/rux \
    -append "root=/dev/vda rw init=/test/printk_flood console=ttyS0 dfx=periodic"
echo "pw-flood-riscv: qemu exited code=$? (log: $LOG)"
