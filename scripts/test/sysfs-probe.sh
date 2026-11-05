#!/bin/bash
# sysfs-probe.sh — one boot, three listings: /sys, /sys/fs, /sys/fs/pstore.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="${2:-$ROOT/run/sysfs-probe-serial.log}"
IMG="$ROOT/work/trace-smoke2.img"
BOOT_DELAY="${BOOT_DELAY:-30}"
TIMEOUT="${TIMEOUT:-120}"
mkdir -p "$ROOT/run"; rm -f "$LOG"

{
    sleep "$BOOT_DELAY"
    echo 'echo ===PROBE: /sys ==='
    sleep 0.3
    echo 'ls /sys/'
    sleep 0.8
    echo 'echo ===PROBE: kernel dir ==='
    sleep 0.3
    echo 'ls /sys/kernel/'
    sleep 0.8
    echo 'echo ===PROBE: power state file ==='
    sleep 0.3
    echo 'cat /sys/power/state'
    sleep 0.8
    echo 'echo ===PROBE: module dir ==='
    sleep 0.3
    echo 'ls /sys/module/ | head -4'
    sleep 0.8
    echo 'echo ===PROBE: /sys/fs ==='
    sleep 0.3
    echo 'ls /sys/fs/'
    sleep 0.8
    echo 'echo ===PROBE: /sys/fs/pstore ==='
    sleep 0.3
    echo 'ls -l /sys/fs/pstore/'
    sleep 1
    echo 'echo ===PROBE: done ==='
    sleep 2
} | timeout "$TIMEOUT" qemu-system-riscv64 \
    -M virt -accel tcg,thread=single \
    -cpu rv64,zbb=true,zba=true,zbs=true \
    -m 1G -smp 2 -display none -serial stdio -snapshot \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -kernel "$KERNEL" \
    -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
    > "$LOG" 2>&1
echo "log: $LOG"
