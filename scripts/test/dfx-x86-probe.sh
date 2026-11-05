#!/bin/bash
# dfx-x86-probe.sh — x86_64 DFX smoke: boots the probe as init
# (init=/test/dfx_x86_probe) on the x86 kernel and asserts every DFX
# procfs surface dumps without a panic, plus a clean power-off.
#
# The probe image is a fresh minimal ext4 (the pstore-x86 recipe:
# mkfs -d over a staging dir holding the static probe; the host gcc
# targets the amd64 guest directly).
#
# -m 2G: the x86 layout's kernel heap starts at the 1 GiB mark (see
# test/pstore-x86-smoke.sh for the ALLOCTHROW-at-1G failure mode).
#
# Usage: test/dfx-x86-probe.sh   (env: LEG_X86_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
: "${LEG_X86_KERNEL:=$ROOT/target/x86_64-unknown-none/debug/bzImage}"
: "${LEG_TIMECAP:=300}"
LOG="$ROOT/run/dfx-x86-probe-serial.log"

x86_result() {  # x86_result PASS|FAIL|SKIP detail
    echo "PROBE-RESULT: $1 $2"
    case "$1" in
        PASS) exit 0 ;;
        SKIP) exit 3 ;;
        *)    exit 1 ;;
    esac
}

[ -f "$LEG_X86_KERNEL" ] || x86_result SKIP "x86 bzImage not built at $LEG_X86_KERNEL (PLATFORM=x86_64 make build)"
command -v gcc >/dev/null 2>&1 || x86_result SKIP "host gcc not found"
MKFS=/usr/sbin/mkfs.ext4
[ -x "$MKFS" ] || MKFS=$(command -v mkfs.ext4) || x86_result SKIP "mkfs.ext4 not found"

PROBE="$ROOT/work/dfx-x86-px"
STAGING="$ROOT/work/dfx-x86-root"
IMG="$ROOT/work/dfx-x86.img"
mkdir -p "$ROOT/run" "$ROOT/work"
gcc -static -O1 -o "$PROBE" "$HERE/dfx_x86_probe.c" || x86_result FAIL "probe build failed"
rm -rf "$STAGING"
mkdir -p "$STAGING/test"
cp "$PROBE" "$STAGING/test/dfx_x86_probe"
chmod 755 "$STAGING/test/dfx_x86_probe"
rm -f "$IMG"
"$MKFS" -q -F -O '^metadata_csum' -d "$STAGING" "$IMG" 96M || x86_result FAIL "mkfs failed"

rm -f "$LOG"
sleep "$LEG_TIMECAP" | timeout "$LEG_TIMECAP" qemu-system-x86_64 \
    -M q35 -accel tcg,thread=multi -cpu max \
    -m 2G -smp 2 -display none -serial stdio \
    -snapshot \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,drive=rootfs \
    -kernel "$LEG_X86_KERNEL" \
    -append "root=/dev/vda rw init=/test/dfx_x86_probe console=ttyS0" \
    > "$LOG" 2>&1

echo "serial log: $LOG"
if grep -aEq "KERNPANIC|Kernel panic|panic!|Oops:" "$LOG"; then
    grep -aE "KERNPANIC|Kernel panic|panic!|Oops:" "$LOG" | head -3
    x86_result FAIL "kernel panic/oops in the x86 serial log"
fi
for marker in "===XSMOKE: start" "===XSMOKE: /proc/stat" \
              "===XSMOKE: /proc/vmstat" "===XSMOKE: /proc/schedstat" \
              "===XSMOKE: /proc/softirqs" "===XSMOKE: /proc/buddyinfo" \
              "===XSMOKE: /proc/kallsyms" "===XSMOKE: done"; do
    grep -aq "$marker" "$LOG" || x86_result FAIL "missing marker: $marker"
done
if grep -aq "(open failed)" "$LOG"; then
    x86_result FAIL "a DFX surface failed to open ($(grep -a '(open failed)' "$LOG" | head -1))"
fi
x86_result PASS "all DFX procfs surfaces dumped, clean power-off"
