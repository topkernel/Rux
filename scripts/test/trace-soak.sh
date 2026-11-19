#!/bin/bash
# trace-soak.sh — the Tranche 4 ring soak: tracing on, every event
# armed, a churning workload for the soak window, then the ring state
# is read through every view.
#
# Acceptance (plan doc T4): no kernel panic during the soak; the
# per-CPU rings wrap (entries at capacity, overrun > 0 — the
# overwrite path exercised); the trace and trace_pipe views render
# coherent lines afterwards; the buffer clears.
#
# Usage: test/trace-soak.sh [kernel] [logfile]
# Env:   BOOT_DELAY (default 30), SOAK_SECS (default 600)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="${2:-$ROOT/run/trace-soak-serial.log}"
BOOT_DELAY="${BOOT_DELAY:-30}"
SOAK_SECS="${SOAK_SECS:-600}"
TIMEOUT=$((BOOT_DELAY + SOAK_SECS + 180))

mkdir -p "$ROOT/run"
rm -f "$LOG"

{
    sleep "$BOOT_DELAY"
    echo 'echo ===SOAK: arm everything ==='
    sleep 0.3
    echo 'echo 1 > /sys/kernel/debug/tracing/events/enable'
    sleep 0.3
    echo 'echo 1 > /sys/kernel/debug/tracing/tracing_on'
    sleep 0.3
    echo 'echo SOAK-START > /sys/kernel/debug/tracing/trace_marker'
    sleep 0.3
    echo 'echo ===SOAK: churning ==='
    # The workload: a rolling fork/exec/read loop that exercises
    # wakeups, forks, exits, syscalls and faults for the whole window.
    echo 'i=0; while [ $i -lt 100000 ]; do ls / >/dev/null 2>&1; cat /proc/self/maps >/dev/null 2>&1; i=$((i+1)); done &'
    sleep "$SOAK_SECS"
    echo 'echo ===SOAK: state after soak ==='
    sleep 0.5
    echo 'echo 0 > /sys/kernel/debug/tracing/tracing_on'
    sleep 0.5
    echo 'cat /sys/kernel/debug/tracing/per_cpu/cpu0/stats'
    sleep 0.5
    echo 'head -3 /sys/kernel/debug/tracing/per_cpu/cpu0/trace'
    sleep 0.5
    echo 'tail -3 /sys/kernel/debug/tracing/per_cpu/cpu0/trace'
    sleep 0.5
    echo 'wc -l /sys/kernel/debug/tracing/trace'
    sleep 1
    echo 'head -3 /sys/kernel/debug/tracing/trace_pipe'
    sleep 0.5
    echo 'cat /sys/kernel/debug/tracing/per_cpu/cpu1/stats'
    sleep 0.5
    echo 'echo ===SOAK: done ==='
    sleep 3
} | timeout "$TIMEOUT" qemu-system-riscv64 \
    -M virt \
    -accel tcg,thread=single \
    -cpu rv64,zbb=true,zba=true,zbs=true \
    -m 1G \
    -smp 2 \
    -display none \
    -serial stdio \
    -snapshot \
    -drive file="$ROOT/test/rootfs.img",if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -kernel "$KERNEL" \
    -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
    > "$LOG" 2>&1

echo "serial log: $LOG"
grep -n "===SOAK\|KERNPANIC\|panic" "$LOG" | head -20
