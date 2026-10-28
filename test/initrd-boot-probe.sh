#!/bin/bash
# initrd-boot-probe.sh — suite wrapper for the initrd boot-path suite
# (test/initrd/): packs the tiny initramfs (mkinitrd.py) if needed,
# boots it root=/dev/ram0 with NO disk, and requires every marker of
# run-initrd.sh (init run, cmdline tokens, devtmpfs overmount, mknod).
#
# Usage: test/initrd-boot-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=240}"

leg_need_kernel
command -v qemu-system-riscv64 >/dev/null 2>&1 || leg_result SKIP "qemu-system-riscv64 not found"
command -v riscv64-linux-gnu-gcc >/dev/null 2>&1 || leg_result SKIP "riscv64-linux-gnu-gcc not found"

INITRD="$LEG_ROOT/test/initrd/initrd.cpio.gz"
if [ ! -f "$INITRD" ]; then
    python3 "$LEG_ROOT/test/initrd/mkinitrd.py" "$INITRD" > "$LEG_LOGDIR/initrd-mk.log" 2>&1 \
        || leg_result FAIL "mkinitrd.py failed (see run/initrd-mk.log)"
fi

mkdir -p "$LEG_LOGDIR"
: > "$LEG_LOG"
DURATION=$(( LEG_TIMECAP - 60 ))
bash "$LEG_ROOT/test/initrd/run-initrd.sh" "$INITRD" "$DURATION" >> "$LEG_LOG" 2>&1

if grep -aEq "KERNPANIC|Kernel panic|panic!|Oops:" "$LEG_LOG"; then
    leg_result FAIL "kernel panic/oops in the initrd boot"
fi
fails=$(grep -ac "^FAIL: " "$LEG_LOG" || true)
passes=$(grep -ac "^PASS: " "$LEG_LOG" || true)
if [ "$fails" = 0 ] && [ "$passes" -ge 10 ]; then
    leg_result PASS "initrd boot: all $passes markers seen"
fi
leg_result FAIL "initrd boot: $fails failing markers ($passes passing)"
