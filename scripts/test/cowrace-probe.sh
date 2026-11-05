#!/bin/bash
# cowrace-probe.sh — suite wrapper for the COW fork race (the fake-OOM
# "pagefault: Out of memory" repro, layer 2): 400 rounds of
# fork+re-COW with 4 threads hammering the same page at SMP2.
# Builds the pthread probe against the musl SDK, packs a tiny private
# ext4 (init=/test/cow_race, the binder-driver pattern) and asserts
# COWRACE-PASS.
#
# Usage: test/cowrace-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=420}"

leg_need_kernel
command -v /usr/sbin/mkfs.ext4 >/dev/null 2>&1 || command -v mkfs.ext4 >/dev/null 2>&1 || leg_result SKIP "mkfs.ext4 not found"

NAME=cowrace
BIN="$LEG_ROOT/work/leg-cowrace"
STAGING="$LEG_ROOT/work/leg-cowrace-root"
IMG="$LEG_ROOT/work/cowrace-leg.img"
leg_build_musl test/cowrace/cow_race.c "$BIN" -pthread

rm -rf "$STAGING"
mkdir -p "$STAGING/test"
cp "$BIN" "$STAGING/test/cow_race"
chmod 755 "$STAGING/test/cow_race"
MKFS=$(command -v mkfs.ext4 || echo /usr/sbin/mkfs.ext4)
rm -f "$IMG"
"$MKFS" -q -F -O '^metadata_csum,^flex_bg' -d "$STAGING" "$IMG" 64M \
    || leg_result FAIL "mkfs for the cowrace image failed"

: > "$LEG_LOG"
timeout "$LEG_TIMECAP" qemu-system-riscv64 $(leg_qemu_riscv_flags) \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -kernel "$LEG_KERNEL" \
    -append "root=/dev/vda rw init=/test/cow_race console=ttyS0" \
    > "$LEG_LOG" 2>&1
rc=$?
rm -f "$IMG"
tail -5 "$LEG_LOG"
[ "$rc" = 0 ] || [ "$rc" = 124 ] || leg_result FAIL "qemu exited rc=$rc"
leg_panic_check
if grep -aq "COWRACE-PASS" "$LEG_LOG"; then
    leg_result PASS "COWRACE-PASS seen"
fi
leg_result FAIL "no COWRACE-PASS in serial log (fake-OOM COW race shape or early death)"
