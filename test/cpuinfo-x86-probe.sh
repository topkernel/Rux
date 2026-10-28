#!/bin/bash
# cpuinfo-x86-probe.sh — the x86 /proc/cpuinfo block-shape contract.
#
# Boots the x86 kernel with test/cpuinfo_x86probe.c as init (fresh
# minimal ext4, the dfx-x86-probe recipe) and asserts:
#   * `processor` appears exactly once per CPU block (Linux
#     arch/x86/kernel/cpu/proc.c emits it once, as the block's first
#     row; Rux used to duplicate it after cache_alignment);
#   * no block carries `processor` after cache_alignment;
#   * the file starts with the first block's processor row.
# Serial must stay clean (no panic/Oops) and the probe must power off.
#
# Usage: test/cpuinfo-x86-probe.sh   (env: LEG_X86_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
: "${LEG_X86_KERNEL:=$ROOT/target/x86_64-unknown-none/debug/bzImage}"
: "${LEG_TIMECAP:=300}"
LOG="$ROOT/run/cpuinfo-x86-probe-serial.log"

x86_result() {  # PASS|FAIL|SKIP detail
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

PROBE="$ROOT/work/cpuinfo-x86-px"
STAGING="$ROOT/work/cpuinfo-x86-root"
IMG="$ROOT/work/cpuinfo-x86.img"
mkdir -p "$ROOT/run" "$ROOT/work"
gcc -static -O1 -o "$PROBE" "$HERE/cpuinfo_x86probe.c" || x86_result FAIL "probe build failed"
rm -rf "$STAGING"
mkdir -p "$STAGING/test"
cp "$PROBE" "$STAGING/test/cpuinfo_x86probe"
chmod 755 "$STAGING/test/cpuinfo_x86probe"
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
    -append "root=/dev/vda rw init=/test/cpuinfo_x86probe console=ttyS0" \
    > "$LOG" 2>&1

echo "serial log: $LOG"
if grep -aEq "KERNPANIC|Kernel panic|panic!|Oops:" "$LOG"; then
    grep -aE "KERNPANIC|Kernel panic|panic!|Oops:" "$LOG" | head -3
    x86_result FAIL "kernel panic/oops in the x86 serial log"
fi
grep -aq "===CPUINFO-X86: start" "$LOG" || x86_result FAIL "probe never started"
grep -aq "PROBE-RESULT: PASS" "$LOG" || x86_result FAIL "cpuinfo block-shape contract failed ($(grep -a 'PROBE-FAIL' "$LOG" | head -1))"
x86_result PASS "processor once per block, no trailing duplicate"
