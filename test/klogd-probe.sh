#!/bin/bash
# klogd-probe.sh — one-boot diagnostic for the klogd periodic flush:
# does the klogd kthread exist, what state is it in, and does
# /var/log/kmsg ever grow past the init flush?
#
# Timeline: boot (30 s), sysrq 't' task dump (every task's name+state),
# count kmsg lines, wait 35 s (past klogd's first 30 s wake), count
# again, dump tasks again.
#
# Usage: test/klogd-probe.sh [kernel] [logfile]

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="${2:-$ROOT/run/klogd-probe-serial.log}"

IMG="$ROOT/work/klogd-probe.img"

mkdir -p "$ROOT/run" "$ROOT/work"
rm -f "$LOG" "$IMG"
cp "$ROOT/test/rootfs.img" "$IMG"

{
    sleep 30
    echo 'echo ===KP: boot done, task dump 1 ==='
    sleep 0.5
    echo 'echo t > /proc/sysrq-trigger'
    sleep 3
    echo 'echo ===KP: kmsg lines t0 ==='
    sleep 0.3
    echo 'wc -l /var/log/kmsg'
    sleep 0.5
    echo 'ls -l /var/log/kmsg'
    sleep 0.5
    echo 'echo ===KP: waiting 35s past klogd first wake ==='
    sleep 35
    echo 'wc -l /var/log/kmsg'
    sleep 0.5
    echo 'echo ===KP: task dump 2 ==='
    sleep 0.3
    echo 'echo t > /proc/sysrq-trigger'
    sleep 3
    echo 'echo ===KP: done ==='
    sleep 2
} | timeout 150 qemu-system-riscv64 \
    -M virt \
    -accel tcg,thread=single \
    -cpu rv64,zbb=true,zba=true,zbs=true \
    -m 1G \
    -smp 2 \
    -display none \
    -serial stdio \
    -snapshot \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -kernel "$KERNEL" \
    -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
    > "$LOG" 2>&1

echo "serial log: $LOG"
