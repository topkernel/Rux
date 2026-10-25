#!/bin/bash
# tracefs-smoke.sh — boot the minimal rootfs and exercise the tracefs
# surface (Tranche 4) plus the timer_list/sysrq alignment fixes from
# the guest shell.
#
# Checks, in order:
#   1. the tracing dir lists the standard file set
#   2. available_events lists every event as group:name
#   3. the format files match the live kernel's shape
#   4. enable/set_event state transitions incl. the 1/0/X convention
#   5. a recorded trace: header bytes, record line geometry,
#      raw_syscalls + sched_switch lines
#   6. trace_pipe consumes the buffer
#   7. /proc/timer_list: all 8 clock bases, per-CPU sections
#   8. /proc/sysrq-trigger: mode 0200, read refused
#   9. the expanded event set: sched_waking/sched_wakeup/
#      sched_wakeup_new/sched_process_fork/sched_process_exit and
#      exceptions:page_fault_user/page_fault_kernel — format files,
#      set_event arming, recorded lines (page faults render %ps
#      symbolically through the kernel symbol table)
#
# Usage: test/tracefs-smoke.sh [kernel] [logfile]
# Env:   BOOT_DELAY (default 30), TIMEOUT (default 300)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="${2:-$ROOT/run/tracefs-smoke-serial.log}"
BOOT_DELAY="${BOOT_DELAY:-30}"
TIMEOUT="${TIMEOUT:-300}"

mkdir -p "$ROOT/run"
rm -f "$LOG"

{
    sleep "$BOOT_DELAY"
    echo 'echo ===TRACE: file set ==='
    sleep 0.3
    echo 'ls /sys/kernel/debug/tracing'
    sleep 0.3
    echo 'echo ===TRACE: available_events ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/available_events'
    sleep 0.3
    echo 'echo ===TRACE: tracers ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/available_tracers /sys/kernel/debug/tracing/current_tracer /sys/kernel/debug/tracing/buffer_size_kb'
    sleep 0.3
    echo 'echo ===TRACE: sys_enter format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/raw_syscalls/sys_enter/format'
    sleep 0.3
    echo 'echo ===TRACE: sys_exit format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/raw_syscalls/sys_exit/format'
    sleep 0.3
    echo 'echo ===TRACE: sched_switch format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/sched/sched_switch/format'
    sleep 0.3
    echo 'echo ===TRACE: id + initial enable state ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/raw_syscalls/sys_enter/id /sys/kernel/debug/tracing/events/raw_syscalls/enable /sys/kernel/debug/tracing/events/enable /sys/kernel/debug/tracing/tracing_on'
    sleep 0.3
    echo 'echo ===TRACE: set_event arm one ==='
    sleep 0.3
    echo 'echo raw_syscalls:sys_enter > /sys/kernel/debug/tracing/set_event'
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/set_event'
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/raw_syscalls/enable /sys/kernel/debug/tracing/events/enable'
    sleep 0.3
    echo 'echo ===TRACE: set_event disarm ==='
    sleep 0.3
    echo 'echo "!raw_syscalls:sys_enter" > /sys/kernel/debug/tracing/set_event'
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/set_event /sys/kernel/debug/tracing/events/enable'
    sleep 0.3
    echo 'echo ===TRACE: record ==='
    sleep 0.3
    echo 'echo 1 > /sys/kernel/debug/tracing/events/enable'
    sleep 0.3
    echo 'echo 1 > /sys/kernel/debug/tracing/tracing_on'
    sleep 0.3
    echo 'ls / > /dev/null'
    sleep 1
    echo 'echo 0 > /sys/kernel/debug/tracing/tracing_on'
    sleep 0.5
    echo 'head -25 /sys/kernel/debug/tracing/trace'
    sleep 0.5
    echo 'echo ===TRACE: sched_switch lines ==='
    sleep 0.3
    echo 'grep sched_switch /sys/kernel/debug/tracing/trace | head -3'
    sleep 0.5
    echo 'echo ===TRACE: sys_enter lines ==='
    sleep 0.3
    echo 'grep "sys_enter" /sys/kernel/debug/tracing/trace | head -3'
    sleep 0.5
    echo 'echo ===TRACE: trace_pipe consumes ==='
    sleep 0.3
    echo 'wc -l < /sys/kernel/debug/tracing/trace'
    sleep 0.3
    echo 'head -3 /sys/kernel/debug/tracing/trace_pipe'
    sleep 0.5
    echo 'wc -l < /sys/kernel/debug/tracing/trace'
    sleep 0.5
    echo 'echo ===TRACE: filter files ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/sched/sched_switch/filter /sys/kernel/debug/tracing/events/sched/filter'
    sleep 0.3
    echo 'echo ===EVENTS: new event set ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/available_events'
    sleep 0.5
    echo 'echo ===EVENTS: sched_wakeup format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/sched/sched_wakeup/format'
    sleep 0.5
    echo 'echo ===EVENTS: page_fault_user format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/exceptions/page_fault_user/format'
    sleep 0.5
    echo 'echo ===EVENTS: arm and generate ==='
    sleep 0.3
    echo 'echo sched:sched_waking sched:sched_wakeup sched:sched_wakeup_new sched:sched_process_fork sched:sched_process_exit exceptions:page_fault_user exceptions:page_fault_kernel > /sys/kernel/debug/tracing/set_event'
    sleep 0.3
    echo 'echo 1 > /sys/kernel/debug/tracing/tracing_on'
    sleep 0.3
    echo 'ls / >/dev/null; ls /proc >/dev/null; cat /proc/self/maps >/dev/null'
    sleep 1
    echo 'echo 0 > /sys/kernel/debug/tracing/tracing_on'
    sleep 0.5
    echo 'echo ===EVENTS: wakeup family lines ==='
    sleep 0.3
    echo 'grep sched_wakeup: /sys/kernel/debug/tracing/trace | head -4'
    sleep 0.5
    echo 'echo ===EVENTS: fork and exit lines ==='
    sleep 0.3
    echo 'grep sched_process_fork: /sys/kernel/debug/tracing/trace | head -2; grep sched_process_exit: /sys/kernel/debug/tracing/trace | head -2'
    sleep 0.5
    echo 'echo ===EVENTS: page fault lines ==='
    sleep 0.3
    echo 'grep page_fault_user: /sys/kernel/debug/tracing/trace | head -4'
    sleep 0.5
    echo 'echo 0 > /sys/kernel/debug/tracing/events/enable'
    sleep 0.5
    echo 'echo ===TRACE: timer_list shape ==='
    sleep 0.3
    echo 'head -22 /proc/timer_list'
    sleep 0.5
    echo 'grep -c "^clock" /proc/timer_list'
    sleep 0.3
    echo 'echo ===TRACE: sysrq mode ==='
    sleep 0.3
    echo 'ls -l /proc/sysrq-trigger'
    sleep 0.3
    echo 'cat /proc/sysrq-trigger'
    sleep 0.5
    echo 'echo ===TRACE: done ==='
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
grep -n "===TRACE" "$LOG" | head -30
