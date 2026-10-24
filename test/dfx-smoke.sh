#!/bin/bash
# dfx-smoke.sh — boot the minimal rootfs and exercise the new DFX
# surfaces from the guest shell.
#
# Checks, in order:
#   1. /sys/kernel/debug/dfx/ lists one knob per DFX switch
#   2. a knob reads Y/N and can be armed and disarmed at runtime
#   3. /proc/sys/kernel/tainted carries the live taint bitmask
#   4. /proc/sysrq-trigger runs a command (task dump + help)
#   5. /sys/kernel survived the debugfs mount (uevent etc. still there)
#
# Usage: test/dfx-smoke.sh [kernel] [logfile]
# Env:   BOOT_DELAY (default 15), TIMEOUT (default 240)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="${2:-$ROOT/run/dfx-smoke-serial.log}"
BOOT_DELAY="${BOOT_DELAY:-15}"
TIMEOUT="${TIMEOUT:-240}"

mkdir -p "$ROOT/run"
rm -f "$LOG"

{
    sleep "$BOOT_DELAY"
    echo 'echo ===SMOKE: dfx knobs ==='
    sleep 0.3
    echo 'ls /sys/kernel/debug/dfx'
    sleep 0.3
    echo 'echo ===SMOKE: knob read ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/dfx/taskdump'
    sleep 0.3
    echo 'echo ===SMOKE: knob arm ==='
    sleep 0.3
    echo 'echo 1 > /sys/kernel/debug/dfx/memwatch'
    sleep 0.3
    echo 'cat /sys/kernel/debug/dfx/memwatch'
    sleep 0.3
    echo 'echo ===SMOKE: knob disarm ==='
    sleep 0.3
    echo 'echo 0 > /sys/kernel/debug/dfx/memwatch'
    sleep 0.3
    echo 'cat /sys/kernel/debug/dfx/memwatch'
    sleep 0.3
    echo 'echo ===SMOKE: taint ==='
    sleep 0.3
    echo 'cat /proc/sys/kernel/tainted'
    sleep 0.3
    echo 'echo ===SMOKE: sysfs intact ==='
    sleep 0.3
    echo 'ls /sys/kernel'
    sleep 0.3
    echo 'echo ===SMOKE: proc stat ==='
    sleep 0.3
    echo 'cat /proc/stat'
    sleep 0.3
    echo 'echo ===SMOKE: proc self stat ==='
    sleep 0.3
    echo 'cat /proc/self/stat'
    sleep 0.3
    echo 'echo ===SMOKE: proc self status ctxt ==='
    sleep 0.3
    echo 'grep ctxt /proc/self/status'
    sleep 0.3
    echo 'echo ===SMOKE: init shell ctxt ==='
    sleep 0.3
    echo 'grep ctxt /proc/1/status'
    sleep 0.3
    echo 'echo ===SMOKE: init shell stat ==='
    sleep 0.3
    echo 'cat /proc/1/stat'
    sleep 0.3
    echo 'echo ===SMOKE: proc vmstat ==='
    sleep 0.3
    echo 'grep -E "pgfault|pgmajfault|pgfree|pgalloc_normal|nr_free_pages" /proc/vmstat'
    sleep 0.3
    echo 'echo ===SMOKE: proc buddyinfo ==='
    sleep 0.3
    echo 'cat /proc/buddyinfo'
    sleep 0.3
    echo 'echo ===SMOKE: proc softirqs ==='
    sleep 0.3
    echo 'head -4 /proc/softirqs'
    sleep 0.3
    echo 'echo ===SMOKE: proc schedstat ==='
    sleep 0.3
    echo 'cat /proc/schedstat'
    sleep 0.3
    echo 'echo ===SMOKE: proc locks ==='
    sleep 0.3
    echo 'cat /proc/locks'
    sleep 0.3
    echo 'flock /tmp/dfxlock cat /proc/locks'
    sleep 0.3
    echo 'echo ===SMOKE: proc self stat fields ==='
    sleep 0.3
    echo 'cat /proc/self/stat'
    sleep 0.3
    echo 'echo ===SMOKE: proc kallsyms ==='
    sleep 0.3
    echo 'head -3 /proc/kallsyms'
    sleep 0.3
    echo 'grep -m 3 "rux::" /proc/kallsyms'
    sleep 0.3
    echo 'echo ===SMOKE: symbolized backtrace ==='
    sleep 0.3
    echo 'echo l > /proc/sysrq-trigger'
    sleep 0.3
    sleep 3
    echo 'echo ===SMOKE: sysrq task dump ==='
    sleep 0.3
    echo 'echo t > /proc/sysrq-trigger'
    sleep 0.3
    sleep 4
    echo 'echo ===SMOKE: sysrq help ==='
    sleep 0.3
    echo 'echo h > /proc/sysrq-trigger'
    sleep 0.3
    sleep 3
    echo 'echo ===SMOKE: done ==='
    sleep 0.3
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
grep -n "SMOKE" "$LOG" | head -40
