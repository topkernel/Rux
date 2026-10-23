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
    echo 'ls /sys/kernel/debug/dfx'
    echo 'echo ===SMOKE: knob read ==='
    echo 'cat /sys/kernel/debug/dfx/taskdump'
    echo 'echo ===SMOKE: knob arm ==='
    echo 'echo 1 > /sys/kernel/debug/dfx/memwatch'
    echo 'cat /sys/kernel/debug/dfx/memwatch'
    echo 'echo ===SMOKE: knob disarm ==='
    echo 'echo 0 > /sys/kernel/debug/dfx/memwatch'
    echo 'cat /sys/kernel/debug/dfx/memwatch'
    echo 'echo ===SMOKE: taint ==='
    echo 'cat /proc/sys/kernel/tainted'
    echo 'echo ===SMOKE: sysfs intact ==='
    echo 'ls /sys/kernel'
    echo 'echo ===SMOKE: proc stat ==='
    echo 'cat /proc/stat'
    echo 'echo ===SMOKE: proc self stat ==='
    echo 'cat /proc/self/stat'
    echo 'echo ===SMOKE: proc self status ctxt ==='
    echo 'grep ctxt /proc/self/status'
    echo 'echo ===SMOKE: init shell ctxt ==='
    echo 'grep ctxt /proc/1/status'
    echo 'echo ===SMOKE: init shell stat ==='
    echo 'cat /proc/1/stat'
    echo 'echo ===SMOKE: proc vmstat ==='
    echo 'grep -E "pgfault|pgmajfault|pgfree|pgalloc_normal|nr_free_pages" /proc/vmstat'
    echo 'echo ===SMOKE: proc buddyinfo ==='
    echo 'cat /proc/buddyinfo'
    echo 'echo ===SMOKE: proc softirqs ==='
    echo 'head -4 /proc/softirqs'
    echo 'echo ===SMOKE: proc schedstat ==='
    echo 'cat /proc/schedstat'
    echo 'echo ===SMOKE: sysrq task dump ==='
    echo 'echo t > /proc/sysrq-trigger'
    sleep 4
    echo 'echo ===SMOKE: sysrq help ==='
    echo 'echo h > /proc/sysrq-trigger'
    sleep 3
    echo 'echo ===SMOKE: done ==='
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
