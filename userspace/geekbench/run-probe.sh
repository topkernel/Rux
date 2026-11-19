#!/bin/bash
# run-probe.sh — boot the probe image (Rux kernel or reference kernel)
# with /test/aes_probe as init and capture its serial output.
#
# Usage: run-probe.sh <rux|ref> <serial.log> [outer-timeout-s]
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WT="$(cd "$HERE/../.." && pwd)"
MODE="${1:?rux|ref}"
LOG="${2:?serial log path}"
OUTER="${3:-300}"

mkdir -p "$(dirname "$LOG")"
: > "$LOG"

QPID=""
cleanup() {
    [ -n "$QPID" ] && kill "$QPID" 2>/dev/null
    wait "$QPID" 2>/dev/null
}
trap cleanup EXIT

QARGS=(-M q35 -accel kvm -cpu host -smp 4 -m 4G
    -display none -vga none -serial "file:$LOG" -monitor none
    -drive "file=$WT/work/probe-x86.img,if=none,id=rootfs,format=raw,file.locking=off"
    -device virtio-blk-pci,drive=rootfs)

if [ "$MODE" = "rux" ]; then
    QARGS+=(-kernel "$WT/work/bzImage"
        -append "root=/dev/vda rw init=/test/aes_probe console=ttyS0")
else
    # Reference-kernel control: the host's generic kernel, NO initrd —
    # VIRTIO_BLK/VIRTIO_PCI are builtin (=y), and the host initrd's
    # dracut waits for the HOST root UUID and drops to emergency mode.
    # Copy /boot/vmlinuz-* here first (mode 0600 root) and e2fsck -fy
    # the image after the boot: killing QEMU post-PID1-exit leaves ext4
    # dirty.
    QARGS+=(-kernel "$WT/run/geekbench/vmlinuz-7.0.0-38-generic"
        -append "root=/dev/vda rw init=/test/aes_probe console=ttyS0")
fi

echo "[probe] mode=$MODE log=$LOG"
qemu-system-x86_64 "${QARGS[@]}" &
QPID=$!

deadline=$((SECONDS + OUTER))
rc=3
while [ $SECONDS -lt $deadline ]; do
    if ! kill -0 "$QPID" 2>/dev/null; then
        echo "[probe] QEMU exited"
        rc=1
        break
    fi
    if grep -aq "PROBE done" "$LOG" 2>/dev/null; then
        rc=0
        sleep 3
        break
    fi
    if grep -aqE "KERNEL PANIC" "$LOG" 2>/dev/null; then
        sleep 5
        break
    fi
    sleep 3
done
echo "[probe] done rc=$rc"
exit $rc
