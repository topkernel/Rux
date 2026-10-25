#!/bin/bash
# pstore-smoke.sh — Tranche 5 pstore-lite acceptance: the last panic's
# record survives the reboot and is readable through /sys/fs/pstore.
#
# One QEMU, two boots (a guest reset does not clear RAM):
#   boot 1: panic=3 on the command line; the driver crashes the kernel
#           via sysrq 'c' (echo c > /proc/sysrq-trigger) — the panic
#           report prints, the record is written to the reserved region,
#           and the panic= policy reboots the guest ~3 s later;
#   boot 2: the boot log carries the pstore found-record line, and
#           /sys/fs/pstore/console-ramoops-0 exists, mode 0444, and its
#           content holds the panic identity (the "Panic:" head), the
#           staged report (the "Kernel panic - not syncing" banner and
#           the Call Trace header) and the console tail.
#
# The probe image is a private copy of test/rootfs.img (never the
# shared one).
#
# Usage: test/pstore-smoke.sh [kernel] [logfile]
# Env:   BOOT_DELAY (default 30), TIMEOUT (default 300)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="${2:-$ROOT/run/pstore-smoke-serial.log}"
BOOT_DELAY="${BOOT_DELAY:-30}"
TIMEOUT="${TIMEOUT:-300}"

IMG="$ROOT/work/pstore-smoke.img"

mkdir -p "$ROOT/run" "$ROOT/work"
rm -f "$LOG" "$IMG"
cp "$ROOT/test/rootfs.img" "$IMG"

{
    # ---- boot 1: crash ------------------------------------------------
    sleep "$BOOT_DELAY"
    echo 'echo ===PS: boot1 crashing via sysrq c ==='
    sleep 0.3
    echo 'echo c > /proc/sysrq-trigger'
    sleep 8
    # ---- boot 2: the record resurfaces --------------------------------
    echo 'echo ===PS: boot2 reading the record ==='
    sleep 30
    echo 'echo ===PS: pstore dir ==='
    sleep 0.3
    echo 'ls -l /sys/fs/pstore/'
    sleep 0.5
    echo 'echo ===PS: record head ==='
    sleep 0.3
    echo 'head -6 /sys/fs/pstore/console-ramoops-0'
    sleep 0.5
    echo 'echo ===PS: record has banner ==='
    sleep 0.3
    echo 'echo PSDBG:banner_count=$(grep -c "Kernel panic - not syncing" /sys/fs/pstore/console-ramoops-0)'
    sleep 0.5
    echo 'echo ===PS: record has call trace ==='
    sleep 0.3
    echo 'echo PSDBG:trace_count=$(grep -c "Call Trace" /sys/fs/pstore/console-ramoops-0)'
    sleep 0.5
    echo 'echo ===PS: record has console tail ==='
    sleep 0.3
    echo 'echo PSDBG:tail_count=$(grep -c "boot: static regions" /sys/fs/pstore/console-ramoops-0)'
    sleep 0.5
    echo 'echo ===PS: record tail ==='
    sleep 0.3
    echo 'tail -4 /sys/fs/pstore/console-ramoops-0'
    sleep 1
    echo 'echo ===PS: done ==='
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
    -append "root=/dev/vda rw init=/bin/sh console=ttyS0 panic=3" \
    > "$LOG" 2>&1

echo "serial log: $LOG"

fail=0
check() {
    local what="$1" pat="$2"
    if grep -qE "$pat" "$LOG"; then
        echo "  PASS: $what"
    else
        echo "  FAIL: $what  (pattern: $pat)"
        fail=1
    fi
}

echo "--- verdicts ---"
check "boot1 panics (sysrq c)" 'sysrq: crashing the kernel on request'
check "panic=3 reboots (the boot banner prints twice)" 'RISC-V 64-bit \| POSIX Compatible.*RISC-V 64-bit \| POSIX Compatible'
check "boot2 finds the record (the pstore boot line)" 'pstore: previous boot.s panic record found'
check "record file exists, mode 0444" 'r..r..r.. .* /sys/fs/pstore/console-ramoops-0'
check "record carries the panic identity" 'Panic: sysrq'
check "record carries the printed banner" 'PSDBG:banner_count=[1-9]'
check "record carries the symbolized stack header" 'PSDBG:trace_count=[1-9]'
check "record carries the console tail (boot1's printk records)" 'PSDBG:tail_count=[1-9]'

exit $fail
