#!/bin/bash
# trace-blkdbg.sh — one-shot interactive probe for the block-event gap:
# arms ONLY the block events, generates write+sync traffic, and dumps
# set_event state, /proc/diskstats deltas and the trace grep, so the
# serial log shows whether the counters (same chokepoints) move while
# the trace stays empty — or neither moves.
#
# Usage: test/trace-blkdbg.sh [kernel] [logfile]

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="${2:-$ROOT/run/trace-blkdbg-serial.log}"
IMG="$ROOT/work/trace-smoke2.img"
BOOT_DELAY="${BOOT_DELAY:-30}"
TIMEOUT="${TIMEOUT:-180}"

mkdir -p "$ROOT/run"
rm -f "$LOG"

{
    sleep "$BOOT_DELAY"
    echo 'echo ===DBG: diskstats before ==='
    sleep 0.3
    echo 'cat /proc/diskstats'
    sleep 0.5
    echo 'echo ===DBG: arm block only ==='
    sleep 0.3
    echo 'echo block:block_rq_issue block:block_rq_complete > /sys/kernel/debug/tracing/set_event'
    sleep 0.3
    echo 'echo 1 > /sys/kernel/debug/tracing/tracing_on'
    sleep 0.3
    echo 'echo ===DBG: set_event now ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/set_event'
    sleep 0.5
    echo 'echo ===DBG: write traffic ==='
    sleep 0.3
    echo 'head -c 200000 /bin/toybox > /dbg.tmp; sync'
    sleep 3
    echo 'echo ===DBG: diskstats after ==='
    sleep 0.3
    echo 'cat /proc/diskstats'
    sleep 0.5
    echo 'echo ===DBG: trace lines ==='
    sleep 0.3
    echo 'grep block_rq_ /sys/kernel/debug/tracing/trace | head -8'
    sleep 1
    echo 'echo ===DBG: raw trace head ==='
    sleep 0.3
    echo 'head -12 /sys/kernel/debug/tracing/trace'
    sleep 1
    echo 'echo 0 > /sys/kernel/debug/tracing/tracing_on'
    sleep 0.5
    echo 'echo ===DBG: done ==='
    sleep 2
} | timeout "$TIMEOUT" qemu-system-riscv64 \
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
