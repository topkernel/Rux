#!/bin/bash
# Boot the heapwatch damage-detector probe on the initrd ramfs.
#
# Usage: test/heapwatch/run-boot.sh [seconds] [arch]
#   arch: riscv64 (default; NOTE: kernel at b3586f69 does not compile
#         for riscv64 — dispatch.rs:658/665 i64/u64, the cast is
#         cfg(x86_64)-gated; use x86_64 until integration fixes it)
#         x86_64 (q35, bzImage)
#
# Boots the kernel with -initrd (root=/dev/ram0) and init=/heapwatch:
# the probe mmaps binder-like/stack-like regions + mallocs, checksums
# every page in a loop with fresh-thread + fork churn, and reports the
# first content change (S1), unwritable page (S2) or vanished VMA (S3)
# with a /proc/self/maps snapshot.  Success markers: HEAPWATCH-START and
# repeating HEAPWATCH-BEAT rows.
#
# Resource discipline: single QEMU, no disk, killed by its own timeout;
# no pkill.  Log: test/heapwatch/boot.log (arch-specific: boot-x86_64.log).
set -u

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
PROJECT_ROOT=$(dirname "$(dirname "$SCRIPT_DIR")")
DURATION="${1:-120}"
ARCH="${2:-riscv64}"

if [ "$ARCH" = "x86_64" ]; then
    CC=x86_64-linux-gnu-gcc
    QEMU=qemu-system-x86_64
    KERNEL="$PROJECT_ROOT/target/x86_64-unknown-none/debug/bzImage"
    LOG="$SCRIPT_DIR/boot-x86_64.log"
    QARGS=(-M q35 -m 2G -smp 2 -accel tcg,thread=single -cpu max
           -nographic -serial stdio -monitor none
           -kernel "$KERNEL")
    CMDLINE="root=/dev/ram0 rw init=/heapwatch console=ttyS0"
    INITRD="$SCRIPT_DIR/initrd-hw-x86_64.cpio.gz"
else
    CC=riscv64-linux-gnu-gcc
    QEMU=qemu-system-riscv64
    KERNEL="$PROJECT_ROOT/target/riscv64gc-unknown-none-elf/debug/rux"
    LOG="$SCRIPT_DIR/boot.log"
    QARGS=(-M virt -accel tcg,thread=single
           -cpu rv64,zbb=true,zba=true,zbs=true
           -m 1G -smp 2 -nographic -serial stdio -monitor none
           -kernel "$KERNEL")
    CMDLINE="root=/dev/ram0 rw init=/heapwatch console=ttyS0,115200"
    INITRD="$SCRIPT_DIR/initrd-hw.cpio.gz"
fi

if [ ! -f "$KERNEL" ]; then
    echo "kernel missing: $KERNEL" >&2
    exit 1
fi
if [ ! -f "$INITRD" ]; then
    echo "initrd missing: $INITRD (run: CC=$CC python3 test/heapwatch/mkinitrd-hw.py $INITRD)" >&2
    exit 1
fi

echo "booting heapwatch probe ($ARCH, log: $LOG, ${DURATION}s)..."
timeout --signal=TERM --kill-after=5 "$DURATION" "$QEMU" \
    "${QARGS[@]}" \
    -initrd "$INITRD" \
    -append "$CMDLINE" \
    > "$LOG" 2>&1
rc=$?

echo "--- exit $rc; markers ---"
for m in HEAPWATCH-START HEAPWATCH-BEAT HEAPWATCH-DAMAGE HEAPWATCH-FAULT; do
    if grep -qF "$m" "$LOG"; then
        echo "SEEN: $m"
    fi
done
grep -E "HEAPWATCH-(START|BEAT|DAMAGE|FAULT)" "$LOG" | tail -5
echo "--- last serial lines ---"
tail -15 "$LOG"
