#!/bin/bash
# swap-x86-probe.sh — x86_64 swap-out end-to-end contract (FD-1).
#
# Boots the x86 kernel with test/swap_x86probe.c as init on a disk whose
# ext4 covers only the head (boot swap_init then carves the 256 MB swap
# tail), under enough anonymous pressure to force vmscan swap-out, and
# asserts:
#   * SwapTotal > 0 (the tail carve activated);
#   * SwapFree DROPPED (mm/rmap try_to_unmap_with_swap actually replaced
#     PTEs — the pre-FD-1 Sv39 walk no-oped on the 4-level tree, so this
#     delta was always zero and reclaim never freed a page);
#   * every touched page reads back its pattern after the swap cycle
#     (right leaf slot written; swap-in restores data);
#   * serial stays clean (no panic/Oops) and the probe powers off.
#
# Usage: test/swap-x86-probe.sh   (env: LEG_X86_KERNEL, LEG_TIMECAP,
#       LEG_MEM_MB, LEG_ACCEL)
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
: "${LEG_X86_KERNEL:=$ROOT/target/x86_64-unknown-none/debug/bzImage}"
: "${LEG_TIMECAP:=420}"
: "${LEG_MEM_MB:=2048}"
: "${LEG_ACCEL:=kvm}"
LOG="$ROOT/run/swap-x86-probe-serial.log"

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
[ -e /dev/kvm ] || LEG_ACCEL=tcg

PROBE="$ROOT/work/swap-x86-px"
STAGING="$ROOT/work/swap-x86-root"
FSIMG="$ROOT/work/swap-x86-fs.img"
IMG="$ROOT/work/swap-x86-disk.img"
mkdir -p "$ROOT/run" "$ROOT/work"
gcc -static -O1 -o "$PROBE" "$HERE/swap_x86probe.c" || x86_result FAIL "probe build failed"
rm -rf "$STAGING"
mkdir -p "$STAGING/test"
cp "$PROBE" "$STAGING/test/swap_x86probe"
chmod 755 "$STAGING/test/swap_x86probe"
# 128 MB ext4 head inside a 512 MB disk: the fs superblock ends well
# before the 256 MB tail swap_init carves (the overlap guard refuses a
# whole-disk fs — mkrootfs images fill the disk and swap stays off).
rm -f "$FSIMG" "$IMG"
"$MKFS" -q -F -O '^metadata_csum' -d "$STAGING" "$FSIMG" 128M || x86_result FAIL "mkfs failed"
truncate -s 512M "$IMG"
dd if="$FSIMG" of="$IMG" conv=notrunc status=none || x86_result FAIL "image layout failed"

rm -f "$LOG"
sleep "$LEG_TIMECAP" | timeout "$LEG_TIMECAP" qemu-system-x86_64 \
    -M q35 -accel "$LEG_ACCEL" -cpu max \
    -m "$LEG_MEM_MB" -smp 2 -display none -serial stdio \
    -snapshot \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,drive=rootfs \
    -kernel "$LEG_X86_KERNEL" \
    -append "root=/dev/vda rw init=/test/swap_x86probe console=ttyS0" \
    > "$LOG" 2>&1

echo "serial log: $LOG"
if grep -aEq "KERNPANIC|Kernel panic|panic!|Oops:" "$LOG"; then
    grep -aE "KERNPANIC|Kernel panic|panic!|Oops:" "$LOG" | head -3
    x86_result FAIL "kernel panic/oops in the x86 serial log"
fi
grep -aq "===SWAP-X86: start" "$LOG" || x86_result FAIL "probe never started"
grep -aq "PROBE-RESULT: PASS" "$LOG" || x86_result FAIL "swap-out contract failed ($(grep -a 'PROBE-RESULT' "$LOG" | tail -1))"
x86_result PASS "swap-out unmapped and swapped anonymous pages; integrity clean"
