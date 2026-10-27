#!/bin/bash
# perf-gate-regress.sh — riscv64 regression boot gate for the perf
# slice: minimal rootfs, scripted serial (cd/proc + reads), rc=0
# markers, zero panics. Same QEMU shape as trace-smoke3.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="$ROOT/run/perf-gate-regress-serial.log"
rm -f "$LOG"

{
    sleep 25
    echo 'cd /proc && echo GATE-CD-RC=$?'
    sleep 1
    echo 'cat /proc/uptime > /dev/null && echo GATE-PROC-RC=$?'
    sleep 1
    echo 'cat /sys/kernel/debug/tracing/available_events > /dev/null && echo GATE-TRACE-RC=$?'
    sleep 1
    echo 'ls / > /dev/null && echo GATE-LS-RC=$?'
    sleep 1
    echo 'echo GATE-DONE'
    sleep 2
} | timeout 120 qemu-system-riscv64 \
    -M virt -accel tcg,thread=single -cpu rv64,zbb=true,zba=true,zbs=true \
    -m 1G -smp 2 -display none -serial stdio -snapshot \
    -drive file="$ROOT/test/rootfs.img",if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -kernel "$KERNEL" \
    -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
    > "$LOG" 2>&1

fail=0
for m in GATE-CD-RC=0 GATE-PROC-RC=0 GATE-TRACE-RC=0 GATE-LS-RC=0 GATE-DONE; do
    if LC_ALL=C grep -aq "$m" "$LOG"; then
        echo "  PASS: $m"
    else
        echo "  FAIL: $m"
        fail=1
    fi
done
P=$(LC_ALL=C grep -acE "KERNPANIC|Kernel panic|Oops:|BUG:|ALLOCTHROW" "$LOG" || true)
if [ "$P" = "0" ]; then
    echo "  PASS: zero panics"
else
    echo "  FAIL: $P panic-class lines"
    fail=1
fi
exit $fail
