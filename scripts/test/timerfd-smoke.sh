#!/bin/bash
# timerfd-smoke.sh — the timerfd_settime(2) contract.
#
# Boots the kernel on a private copy of test/rootfs.img with
# test/timerfd_probe.c compiled in (musl-static; the shared image is never
# written) and greps the probe's PASS/FAIL line plus the serial-clean
# mandate (no panic/Oops).
#
# Usage: test/timerfd-smoke.sh [kernel] [logfile]
# Env:   BOOT_DELAY (default 30), TIMEOUT (default 420)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="${2:-$ROOT/run/timerfd-smoke-serial.log}"
BOOT_DELAY="${BOOT_DELAY:-30}"
TIMEOUT="${TIMEOUT:-420}"

IMG="$ROOT/work/timerfd-smoke.img"

mkdir -p "$ROOT/run" "$ROOT/work"
rm -f "$LOG" "$IMG"

MUSL="$ROOT/toolchain/riscv64-rux-linux-musl"
GCCINC="$(riscv64-linux-gnu-gcc -print-file-name=include)"
riscv64-linux-gnu-gcc -static -nostdlib -nostdinc \
    -isystem "$MUSL/include" -isystem "$GCCINC" \
    -Os -march=rv64gc -D_FORTIFY_SOURCE=0 -fno-stack-protector \
    -o "$ROOT/work/timerfd_probe" \
    "$MUSL/lib/crt1.o" "$ROOT/test/timerfd_probe.c" \
    -L"$MUSL/lib" -lc -lgcc || { echo "probe build failed"; exit 1; }

cp "$ROOT/test/rootfs.img" "$IMG"
debugfs -w -R "rm /test/timerfd_probe" "$IMG" >/dev/null 2>&1
debugfs -w -R "rm /timerfd_probe" "$IMG" >/dev/null 2>&1
# Write at the fs root, then hard-link into /test and drop the temp name
# (the trace-smoke3 inject pattern — writing into /test directly leaves
# an unlinked-flavored inode some kernels refuse to exec).
debugfs -w -R "write $ROOT/work/timerfd_probe /timerfd_probe" "$IMG" >/dev/null 2>&1
INODE=$(debugfs -R "stat /timerfd_probe" "$IMG" 2>/dev/null | sed -n 's/^Inode: \([0-9]*\).*/\1/p' | head -1)
if [ -n "$INODE" ]; then
    debugfs -w -R "ln <${INODE}> /test/timerfd_probe" "$IMG" >/dev/null 2>&1
    debugfs -w -R "unlink /timerfd_probe" "$IMG" >/dev/null 2>&1
fi

{
    sleep "$BOOT_DELAY"
    echo '/test/timerfd_probe'
    sleep 20
    echo 'echo ===TIMERFD-SMOKE-DONE==='
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
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -kernel "$KERNEL" \
    -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
    > "$LOG" 2>&1

echo "serial log: $LOG"

fail=0
check() {
    local what="$1" pat="$2"
    if LC_ALL=C grep -aEq "$pat" "$LOG"; then
        echo "  PASS: $what"
    else
        echo "  FAIL: $what  (pattern: $pat)"
        fail=1
    fi
}

echo "--- verdicts ---"
check "timerfd_settime contract"        'PROBE-RESULT: PASS'
check "probe reached the end"                          'TIMERFD-SMOKE-DONE'
if LC_ALL=C grep -aqE "Kernel panic|Oops:|BUG:" "$LOG"; then
    echo "  FAIL: serial not clean (panic/Oops/BUG present)"
    fail=1
else
    echo "  PASS: serial clean (no panic/Oops/BUG)"
fi

exit $fail
