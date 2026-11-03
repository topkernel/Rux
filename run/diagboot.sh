#!/bin/bash
# diagboot.sh — one diagnostic boot of the lmbench image with a chosen
# probe as init. Usage: diagboot.sh <tag> <kernel> <probe> [smp] [extra-append]
# Artifacts: run/logs/<tag>.log. Boots are -snapshot + file.locking=off.
set -u
TAG="$1"; KERNEL="$2"; PROBE="${3:-diag3}"; SMPN="${4:-2}"; EXTRA="${5:-}"
WT="/home/william/rux-agents/backlog-zero"
IMG="$WT/run/img/lmbench.img"
LOG="$WT/run/logs/$TAG.log"
[ -f "$KERNEL" ] || { echo "kernel missing: $KERNEL" >&2; exit 1; }
timeout 90 qemu-system-x86_64 -M q35 -accel kvm -cpu host \
    -m 2G -smp "$SMPN" -display none -vga none \
    -serial "file:$LOG" -monitor none \
    -pidfile "$WT/run/logs/$TAG.pid" \
    -kernel "$KERNEL" \
    -append "root=/dev/vda rw init=/test/$PROBE console=ttyS0 $EXTRA" \
    -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
    -device virtio-blk-pci,drive=rootfs \
    -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1
rm -f "$WT/run/logs/$TAG.pid"
echo "boot $TAG done rc=$?"
