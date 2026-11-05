#!/bin/bash
# kallsyms-smoke.sh — boot the minimal rootfs and verify the runtime
# symbol table (Tranche 3, RUX-RISCV-LAYOUT-3): the full-table coverage
# end-to-end.
#
# Checks, in order:
#   1. the boot reaches the shell and the heap reports the new base
#      (0x81400000 — the window/heap/reservation moved together)
#   2. /proc/kallsyms lists ~22k symbols (the FULL table, not the old
#      1MiB-capped ~13k subset)
#   3. coverage reaches the END of .text (the highest addresses resolve —
#      the 1MiB cap used to cover only the first ~1.7MB)
#   4. the previously-broken hot-path families resolve by name
#      (sched/trap/syscall/mm)
#   5. a symbolized kernel backtrace: sysrq 'l' prints frames with
#      name+off/size, no bare '?' frames inside the kernel text window
#
# Usage: test/kallsyms-smoke.sh [kernel] [logfile]
# Env:   BOOT_DELAY (default 30), TIMEOUT (default 240)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="${2:-$ROOT/run/kallsyms-smoke-serial.log}"
BOOT_DELAY="${BOOT_DELAY:-30}"
TIMEOUT="${TIMEOUT:-240}"

mkdir -p "$ROOT/run"
rm -f "$LOG"

{
    sleep "$BOOT_DELAY"
    echo 'echo ===KSYMS: heap base ==='
    sleep 0.3
    echo 'dmesg | grep "heap region"'
    sleep 0.5
    echo 'echo ===KSYMS: kallsyms line count ==='
    sleep 0.3
    echo 'wc -l /proc/kallsyms'
    sleep 1
    echo 'echo ===KSYMS: head ==='
    sleep 0.3
    echo 'head -4 /proc/kallsyms'
    sleep 0.5
    echo 'echo ===KSYMS: tail (end-of-text coverage) ==='
    sleep 0.3
    echo 'tail -4 /proc/kallsyms'
    sleep 0.5
    echo 'echo ===KSYMS: hot-path families ==='
    sleep 0.3
    echo 'grep -c " sched" /proc/kallsyms; grep -m2 "schedule" /proc/kallsyms'
    sleep 0.5
    echo 'grep -m2 "trap" /proc/kallsyms'
    sleep 0.5
    echo 'grep -m2 "syscall" /proc/kallsyms'
    sleep 0.5
    echo 'grep -m2 "page_fault\|handle_mm_fault" /proc/kallsyms'
    sleep 0.5
    echo 'echo ===KSYMS: symbolized backtrace (sysrq l) ==='
    sleep 0.3
    echo 'echo l > /proc/sysrq-trigger'
    sleep 3
    echo 'echo ===KSYMS: done ==='
    sleep 1
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
grep -n "===KSYMS" "$LOG" | head -20
