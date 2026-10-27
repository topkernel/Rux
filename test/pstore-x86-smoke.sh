#!/bin/bash
# pstore-x86-smoke.sh — pstore multi-record + reset-path survival
# acceptance on x86_64 (q35): records survive BOTH x86 reset paths (the
# 8042 keyboard-controller pulse AND the triple fault through a NULL
# IDT — each pinned in isolation via the dfx reset-method knob), the
# ring keeps both, and /sys/fs/pstore reads them back newest-first
# with read-once removal.
#
# One QEMU, three boots (a guest reset does not clear RAM):
#   boot 1: panic=3 on the command line; the /test/px init pins the
#           reset method to "8042" and crashes via sysrq 'c'; the
#           record (seq 1) lands in the carve above the kernel image
#           end and the pulse reboots the guest;
#   boot 2: pins the reset method to "triplefault" and crashes again
#           (seq 2); the triple fault resets the guest;
#   boot 3: reads BOTH records — console-ramoops-0 (boot 2, the
#           triple-fault survivor) and console-ramoops-1 (boot 1, the
#           pulse survivor) — checks their identity, and verifies the
#           read-once consumption (re-open ENOENT, directory empty).
#
# The image is a fresh minimal ext4 (the pbench recipe: mkfs -d over a
# staging dir holding the static probe).
#
# Usage: test/pstore-x86-smoke.sh [bzImage] [logfile]
# Env:   BOOT_DELAY (default 25), TIMEOUT (default 540)
#
# -m 2G, not 1G: the x86 layout's kernel heap starts at the 1 GiB mark,
# so a 1 GiB guest panics during early boot (ALLOCTHROW at main.rs —
# the same failure pbench_boot.py documents for --mem 1G). TIMEOUT 540:
# three boots (two crashes + read-back) under a loaded host need the
# margin the riscv smoke carries.

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BZIMAGE="${1:-$ROOT/target/x86_64-unknown-none/debug/bzImage}"
LOG="${2:-$ROOT/run/pstore-x86-serial.log}"
BOOT_DELAY="${BOOT_DELAY:-25}"
TIMEOUT="${TIMEOUT:-540}"

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
    # boot 1 crashes by itself (init=px); boots 2 and 3 follow the
    # probe's role logic. Nothing to type — the sleeps only pace the
    # log capture.
    sleep "$TIMEOUT"
} | timeout "$TIMEOUT" qemu-system-x86_64 \
    -M q35 \
    -accel tcg,thread=multi \
    -cpu max \
    -m 2G \
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
check "boot1 crashes (sysrq c, 8042 pinned)" 'PX1: crashing via sysrq c, reset pinned to 8042'
check "the panic printed" 'Kernel panic'
check "the pulse reset worked (boot2 reached)" 'PX2: crashing via sysrq c, reset pinned to triplefault'
check "the triple-fault reset worked (boot3 reached)" 'PX3: two records resurfaced'
check "the pstore announce lines (2 records on boot3)" 'pstore: previous boot.s console record found'
banners=$(grep -ac 'POSIX Compatible' "$LOG" 2>/dev/null || echo 0)
if [ "${banners:-0}" -ge 3 ]; then
    echo "  PASS: three boots ran (banner x$banners)"
else
    echo "  FAIL: three boots (banner count ${banners:-0} < 3)"
    fail=1
fi
check "boot3 read the newest record" 'PX3: read console-ramoops-0'
check "boot3 read the oldest record" 'PX3: read console-ramoops-1'
check "the newest record carries the panic banner" 'PX3: newest has panic banner'
check "the newest record carries the Call Trace header" 'PX3: newest has call trace'
check "newest record = boot2 (triple-fault survivor)" 'PX3: newest is boot2'
check "oldest record = boot1 (8042-pulse survivor)" 'PX3: oldest is boot1'
check "read-once removed the consumed record" 'PX3: read-once removed newest'
check "directory empty after both reads" 'PX3: directory empty'
check "boot3 idles cleanly past the dump" 'PX3: record end'

exit $fail
