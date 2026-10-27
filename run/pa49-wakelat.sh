#!/bin/bash
# pa49-wakelat.sh — wake-latency distribution under load, captured with
# the T4 trace events (leg-35 deliverable: the sched wake/fork/exit
# events landed with the DFX batch make syscost-free wake-latency
# measurement possible; this is the first consumer).
#
# Channel: minimal rootfs, init=/bin/sh, commands piped via serial
# (trace-soak.sh pattern). Arms ONLY sched:sched_waking +
# sched:sched_wakeup + sched:sched_switch (minimal event production so
# the 1024-entry per-CPU ring keeps waking/switch pairs together under
# load), runs two fork/exec churn loops (the soak's workload), drains
# trace_pipe to /tmp/wl.txt for CAP_SECS bounded by /proc/uptime, then
# dumps ring stats + the capture to serial.
#
# Host-side parse: run/pa49-wakelat-parse.sh <serial-log>
# Usage: pa49-wakelat.sh [kernel] [tag]
# Env: CAP_SECS (default 45)
set -u
K="${1:-$(cd "$(dirname "$0")/.." && pwd)/target/riscv64gc-unknown-none-elf/debug/rux}"
TAG="${2:-pa49wl}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LOG="$ROOT/run/pa-$TAG-serial.log"
CAP_SECS="${CAP_SECS:-45}"
SETTLE="${SETTLE:-25}"
TIMEOUT=$((SETTLE + CAP_SECS + 360))

T=/sys/kernel/debug/tracing
rm -f "$LOG"

{
    sleep "$SETTLE"
    echo 'echo ===WL: arm sched events ==='
    sleep 0.5
    echo "echo 0 > $T/tracing_on"
    sleep 0.5
    echo "echo 1 > $T/events/sched/sched_waking/enable"
    sleep 0.3
    echo "echo 1 > $T/events/sched/sched_wakeup/enable"
    sleep 0.3
    echo "echo 1 > $T/events/sched/sched_switch/enable"
    sleep 0.3
    echo "echo 1 > $T/tracing_on"
    sleep 0.3
    echo 'echo ===WL: churn on ==='
    # two fork/exec churn loops (wake/fork/exit exerciser)
    echo 'i=0; while [ $i -lt 1000000 ]; do ls / >/dev/null 2>&1; cat /proc/self/maps >/dev/null 2>&1; i=$((i+1)); done &'
    sleep 0.3
    echo 'i=0; while [ $i -lt 1000000 ]; do ls / >/dev/null 2>&1; cat /proc/self/maps >/dev/null 2>&1; i=$((i+1)); done &'
    sleep 0.5
    echo "echo ===WL: capture $CAP_SECS s ==="
    echo "U=\$(cut -d. -f1 /proc/uptime); E=\$((U+$CAP_SECS)); while [ \$(cut -d. -f1 /proc/uptime) -lt \$E ]; do cat $T/trace_pipe >> /tmp/wl.txt; done"
    sleep $((CAP_SECS + 5))
    echo "echo 0 > $T/tracing_on"
    sleep 1
    echo 'echo ===WL-STATS cpu0 ==='
    echo "cat $T/per_cpu/cpu0/stats"
    sleep 1
    echo 'echo ===WL-STATS cpu1 ==='
    echo "cat $T/per_cpu/cpu1/stats"
    sleep 1
    echo 'echo ===WL-SIZE ==='
    echo 'wc -l /tmp/wl.txt'
    sleep 1
    echo 'echo ===WL-DUMP ==='
    echo 'cat /tmp/wl.txt'
    sleep 20
    echo 'echo ===WL-END ==='
    sleep 5
} | setsid nice -n 10 timeout "$TIMEOUT" qemu-system-riscv64 \
    -M virt -accel tcg,thread=multi \
    -cpu rv64,zbb=true,zba=true,zbs=true,zcb=true,v=true \
    -m 1G -smp 2 -display none -serial stdio \
    -snapshot \
    -drive "file=$ROOT/test/rootfs.img,if=none,id=rootfs,format=raw,file.locking=off" \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -kernel "$K" \
    -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
    > "$LOG" 2>&1

echo "serial log: $LOG ($(wc -c < "$LOG") bytes)"
grep -c "===WL" "$LOG" || true
