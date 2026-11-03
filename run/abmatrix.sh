#!/bin/bash
# abmatrix.sh — controlled A/B on the r3a kernel binary: unpatched
# (write_bytes count bug zeroes 0x6e40 bytes) vs binary-patched
# (0xa8) — identical layout, only the store length differs.
# Usage: abmatrix.sh <kernel> <label> <runs>
set -u
KERN="$1"; LABEL="$2"; RUNS="${3:-6}"
WT="/home/william/rux-agents/backlog-zero"
IMG="$WT/run/img/lmbench.img"
for i in $(seq 1 "$RUNS"); do
    LOG="$WT/run/logs/ab-${LABEL}-$i.log"
    timeout 90 qemu-system-x86_64 -M q35 -accel kvm -cpu host \
        -m 2G -smp 2 -display none -vga none -serial "file:$LOG" -monitor none \
        -kernel "$KERN" \
        -append "root=/dev/vda rw init=/test/diag3 console=ttyS0" \
        -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
        -device virtio-blk-pci,drive=rootfs \
        -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1
    # verdict: block-280 rows are corrupt iff lat_pagefault mode=0
    if grep -aq "ino3537 mode=0 " "$LOG"; then
        echo "$LABEL run$i: ZEROED (block 280 corrupt)"
    elif grep -aq "mode=100775 size=217648" "$LOG"; then
        echo "$LABEL run$i: OK"
    else
        echo "$LABEL run$i: INCONCLUSIVE (probe did not run?)"
    fi
done
