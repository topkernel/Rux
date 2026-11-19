#!/bin/bash
# heapwatch-x86-smoke.sh — suite wrapper for the heap damage-detector
# boot (test/heapwatch/, x86_64 leg): boots the prebuilt
# initrd-hw-x86_64.cpio.gz with init=/heapwatch on the q35 kernel for
# 120 s and asserts the probe started and kept beating
# (HEAPWATCH-START + repeating HEAPWATCH-BEAT rows, no damage report).
#
# The riscv64 path of run-boot.sh is not wired: its mkinitrd-hw.py
# builds heapwatch with plain static glibc (the vector-ifunc SIGILL
# class on riscv) and the note in run-boot.sh says the riscv kernel
# compile was broken at b3586f69 — revisit when the musl recipe lands
# there.
#
# Usage: test/heapwatch-x86-smoke.sh   (env: LEG_X86_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
: "${LEG_X86_KERNEL:=$ROOT/target/x86_64-unknown-none/debug/bzImage}"
: "${LEG_TIMECAP:=240}"
LOG="$ROOT/test/heapwatch/boot-x86_64.log"

x86_result() {
    echo "PROBE-RESULT: $1 $2"
    case "$1" in
        PASS) exit 0 ;;
        SKIP) exit 3 ;;
        *)    exit 1 ;;
    esac
}

[ -f "$LEG_X86_KERNEL" ] || x86_result SKIP "x86 bzImage not built at $LEG_X86_KERNEL (PLATFORM=x86_64 make build)"
command -v qemu-system-x86_64 >/dev/null 2>&1 || x86_result SKIP "qemu-system-x86_64 not found"
[ -f "$HERE/heapwatch/initrd-hw-x86_64.cpio.gz" ] || x86_result SKIP "test/heapwatch/initrd-hw-x86_64.cpio.gz absent (rebuild from the heapwatch lane)"

mkdir -p "$ROOT/run"
: > "$LOG"
timeout "$LEG_TIMECAP" bash "$HERE/heapwatch/run-boot.sh" 120 x86_64 > "$ROOT/run/heapwatch-x86-leg.log" 2>&1

if grep -aEq "KERNPANIC|Kernel panic|panic!|Oops:" "$LOG"; then
    grep -aE "KERNPANIC|Kernel panic|panic!|Oops:" "$LOG" | head -3
    x86_result FAIL "kernel panic/oops in the heapwatch boot"
fi
if grep -aq "HEAPWATCH-DAMAGE" "$LOG" || grep -aq "S[123] " "$LOG"; then
    x86_result FAIL "$(grep -a 'HEAPWATCH' "$LOG" | grep -iv beat | head -1)"
fi
if grep -aq "HEAPWATCH-START" "$LOG"; then
    beats=$(grep -ac "HEAPWATCH-BEAT" "$LOG")
    [ "$beats" -ge 2 ] || x86_result FAIL "heapwatch started but only $beats beat(s) in 120s"
    x86_result PASS "HEAPWATCH-START + $beats beats, no damage report"
fi
x86_result FAIL "no HEAPWATCH-START in the serial log"
