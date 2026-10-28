#!/bin/bash
# ohlite-x86-smoke.sh — suite wrapper for the OH-lite boot gate
# (test/ohlite/): builds the 4-image set with host gcc (no OH tree
# needed), boots it through test/run-oh.sh on the x86 kernel and
# asserts the second-stage init gate: "OHLITE: BOOT GATE PASS:
# second-stage init stayed up 60s".
#
# Usage: test/ohlite-x86-smoke.sh   (env: LEG_X86_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
: "${LEG_X86_KERNEL:=$ROOT/target/x86_64-unknown-none/debug/bzImage}"
# boot (~30s) + 60s gate + margin
: "${LEG_TIMECAP:=220}"
LOG="$ROOT/run/ohlite-x86-serial.log"

x86_result() {
    echo "PROBE-RESULT: $1 $2"
    case "$1" in
        PASS) exit 0 ;;
        SKIP) exit 3 ;;
        *)    exit 1 ;;
    esac
}

[ -f "$LEG_X86_KERNEL" ] || x86_result SKIP "x86 bzImage not built at $LEG_X86_KERNEL (PLATFORM=x86_64 make build)"
command -v gcc >/dev/null 2>&1 || x86_result SKIP "host gcc not found"
command -v qemu-system-x86_64 >/dev/null 2>&1 || x86_result SKIP "qemu-system-x86_64 not found"

# Build the image set (fresh each run; ~10 s of host gcc + mkfs).
bash "$HERE/ohlite/mkohlite.sh" > "$ROOT/run/ohlite-build.log" 2>&1 \
    || x86_result FAIL "mkohlite.sh failed (see run/ohlite-build.log)"
for f in ramdisk.img system.img vendor.img; do
    [ -f "$ROOT/work/ohlite/images/$f" ] || x86_result FAIL "mkohlite.sh produced no $f"
done

mkdir -p "$ROOT/run"
rm -f "$LOG"
KERNEL="$LEG_X86_KERNEL" timeout "$LEG_TIMECAP" \
    bash "$HERE/run-oh.sh" "$ROOT/work/ohlite/images" console > "$LOG" 2>&1
rc=$?
tail -5 "$LOG"
[ "$rc" = 0 ] || [ "$rc" = 124 ] || x86_result FAIL "run-oh.sh exited rc=$rc"

if grep -aEq "KERNPANIC|Kernel panic|panic!|Oops:" "$LOG"; then
    grep -aE "KERNPANIC|Kernel panic|panic!|Oops:" "$LOG" | head -3
    x86_result FAIL "kernel panic/oops in the OH-lite boot"
fi
if grep -aq "BOOT GATE PASS" "$LOG"; then
    x86_result PASS "second-stage init stayed up 60s (BOOT GATE PASS)"
fi
x86_result FAIL "OH-lite boot never reached the 60s gate (log: $LOG)"
