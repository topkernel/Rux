#!/bin/bash
# pstore-x86-smoke.sh — Tranche 5 pstore-lite acceptance on x86_64 (q35):
# the last panic's record survives the reboot and reads back through
# /sys/fs/pstore — the x86 twin of test/pstore-smoke.sh, exercising the
# image-end anchor (same address every boot of the same build).
#
# One QEMU, two boots (a guest reset does not clear RAM):
#   boot 1: panic=3 on the command line; the /test/px init crashes the
#           kernel via sysrq 'c'; the record lands in the carve above
#           the kernel image end and panic=3 reboots the guest;
#   boot 2: the same init finds /sys/fs/pstore/console-ramoops-0 and
#           prints its head on the serial console.
#
# The image is a fresh minimal ext4 (the pbench recipe: mkfs -d over a
# staging dir holding the static probe).
#
# Usage: test/pstore-x86-smoke.sh [bzImage] [logfile]
# Env:   BOOT_DELAY (default 25), TIMEOUT (default 300)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BZIMAGE="${1:-$ROOT/target/x86_64-unknown-none/debug/bzImage}"
LOG="${2:-$ROOT/run/pstore-x86-serial.log}"
BOOT_DELAY="${BOOT_DELAY:-25}"
TIMEOUT="${TIMEOUT:-300}"

MUSL_CC="$ROOT/toolchain/x86_64-rux-linux-musl/bin/musl-gcc"
MKFS=/usr/sbin/mkfs.ext4
IMG="$ROOT/work/pstore-x86.img"
STAGING="$ROOT/work/pstore-x86-root"
PROBE="$ROOT/work/pstore-x86-px"

mkdir -p "$ROOT/run" "$ROOT/work"

# Build the static probe.
if ! "$MUSL_CC" -static -O2 -Wall -o "$PROBE" "$ROOT/test/pstore_x86test.c"; then
    echo "probe build failed" >&2
    exit 2
fi

# Pack the rootfs.
rm -rf "$STAGING"
mkdir -p "$STAGING/test"
cp "$PROBE" "$STAGING/test/px"
chmod 755 "$STAGING/test/px"
rm -f "$IMG"
if ! "$MKFS" -q -F -O '^metadata_csum' -d "$STAGING" "$IMG" 96M; then
    echo "mkfs failed" >&2
    exit 2
fi

rm -f "$LOG"
{
    # boot 1 crashes by itself (init=px); boot 2 prints the record and
    # idles. Nothing to type — the sleeps only pace the log capture.
    sleep "$TIMEOUT"
} | timeout "$TIMEOUT" qemu-system-x86_64 \
    -M q35 \
    -accel tcg,thread=multi \
    -cpu max \
    -m 1G \
    -smp 2 \
    -display none \
    -serial stdio \
    -snapshot \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,drive=rootfs \
    -kernel "$BZIMAGE" \
    -append "root=/dev/vda rw init=/test/px console=ttyS0 panic=3" \
    > "$LOG" 2>&1

echo "serial log: $LOG"

fail=0
check() {
    local what="$1" pat="$2"
    if grep -a -qE "$pat" "$LOG"; then
        echo "  PASS: $what"
    else
        echo "  FAIL: $what  (pattern: $pat)"
        fail=1
    fi
}

echo "--- verdicts ---"
check "boot1 crashes (sysrq c)" 'PX1: crashing via sysrq c'
check "the panic printed" 'Kernel panic'
check "panic=3 rebooted (boots >= 2)" 'PX2: record resurfaced'
check "the pstore announce line names the console flavor" "pstore: previous boot.s console record found"
check "the record carries the panic banner" 'Kernel panic - not syncing'
check "the record carries the Call Trace header" 'Call Trace'
check "the record carries the boot-1 sysrq identity" "Panic: sysrq 'c'"
check "the record ends cleanly (boot2 idle past the dump)" 'PX2: record end'

exit $fail
