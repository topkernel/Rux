#!/bin/bash
# run-rux.sh — boot the Rux x86_64 kernel with Geekbench as the payload
# and capture the full run off the serial console (Geekbench's stdout is
# chatty; the score summary block at the end is what gets parsed).
#
# One boot, one full run. The in-guest runner bounds itself with
# gtimeout= (default 900 s below); this script adds an outer wall-clock
# guard and kills only the QEMU PID it started.
#
# Usage: run-rux.sh [bzImage] [img] [serial.log] [outer-timeout-s] [gtimeout-s]
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WT="$(cd "$HERE/../.." && pwd)"
KERNEL="${1:-$WT/target/x86_64-unknown-none/release/bzImage}"
IMG="${2:-$WT/work/geekbench-x86.img}"
LOG="${3:-$WT/run/geekbench/rux-serial.log}"
OUTER="${4:-2700}"          # 45 min wall clock around everything
GTIMEOUT="${5:-900}"        # 15 min in-guest, per the rig spec

mkdir -p "$(dirname "$LOG")"
: > "$LOG"

QPID=""
cleanup() {
    [ -n "$QPID" ] && kill "$QPID" 2>/dev/null
    wait "$QPID" 2>/dev/null
}
trap cleanup EXIT

echo "[boot] kernel=$KERNEL img=$IMG gtimeout=${GTIMEOUT}s outer=${OUTER}s"
qemu-system-x86_64 -M q35 -accel kvm -cpu host -smp 4 -m 4G \
    -display none -vga none -serial "file:$LOG" -monitor none \
    -kernel "$KERNEL" \
    -append "root=/dev/vda rw init=/test/geekbench_run geekbench=cpu gtimeout=$GTIMEOUT console=ttyS0" \
    -drive "file=$IMG,if=none,id=rootfs,format=raw,file.locking=off" \
    -device virtio-blk-pci,drive=rootfs \
    -device virtio-net-pci,netdev=n0 -netdev user,id=n0 &
QPID=$!

deadline=$((SECONDS + OUTER))
rc=3
while [ $SECONDS -lt $deadline ]; do
    if ! kill -0 "$QPID" 2>/dev/null; then
        echo "[boot] QEMU exited early"
        rc=1
        break
    fi
    if grep -aq "GEEKBENCH DONE" "$LOG" 2>/dev/null; then
        rc=0
        sleep 2            # drain the last serial bytes
        break
    fi
    if grep -aqE "KERNEL PANIC|GEEKBENCH TIMEOUT" "$LOG" 2>/dev/null; then
        sleep 5            # let the tail print, then stop the boot
        break
    fi
    sleep 5
done
echo "[boot] done rc=$rc serial=$LOG"
exit $rc
