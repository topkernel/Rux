#!/bin/bash
# panic-policy-smoke.sh — check the `panic=` kernel parameter.
#
# Boots the minimal rootfs with panic=3, triggers a crash through the
# sysrq 'c' command (Linux's "crash" command) and checks that the
# machine reboots on its own: the boot banner has to appear twice in
# the serial log.
#
# Usage: test/panic-policy-smoke.sh [kernel] [logfile]
# Env:   BOOT_DELAY (default 30), PANIC_WAIT (default 20), TIMEOUT (default 240)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="${2:-$ROOT/run/panic-policy-serial.log}"
BOOT_DELAY="${BOOT_DELAY:-30}"
PANIC_WAIT="${PANIC_WAIT:-20}"
TIMEOUT="${TIMEOUT:-240}"

mkdir -p "$ROOT/run"
rm -f "$LOG"

{
    sleep "$BOOT_DELAY"
    # Linux's sysrq 'c': force a crash.
    echo 'echo c > /proc/sysrq-trigger'
    sleep "$PANIC_WAIT"
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
    -append "root=/dev/vda rw init=/bin/sh console=ttyS0 panic=3" \
    > "$LOG" 2>&1

echo "serial log: $LOG"
boots=$(grep -ac "OpenSBI" "$LOG")
panics=$(grep -ac "Kernel panic - not syncing" "$LOG")
echo "boots: $boots   panics: $panics"
if [ "$boots" -ge 2 ] && [ "$panics" -ge 1 ]; then
    echo "PASS: the kernel panicked and rebooted on its own"
    exit 0
fi
echo "FAIL: expected a panic and a second boot"
exit 1
