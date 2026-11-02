#!/bin/bash
# cctx-boot.sh — one cctx-probe boot for one lmbench case.
# Usage: cctx-boot.sh <bzImage> <spec-with-commas> <log>
WT=/home/william/rux-agents/perf-closegap
KERNEL="$1"; SPEC="$2"; LOG="$3"
timeout 60 qemu-system-x86_64 -M q35 -accel kvm -cpu host \
    -m 2G -smp 2 -display none -vga none -serial "file:$LOG" -monitor none \
    -kernel "$KERNEL" \
    -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$SPEC cctx=1 budget=45 console=ttyS0" \
    -drive file="$WT/work/lmbench-x86.img",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
    -device virtio-blk-pci,drive=rootfs >/dev/null 2>&1
grep -aE "CCTX[0-9]|Semaphore|AF_UNIX|Pipe latency|^[0-9]+ [0-9.]+$|Microseconds|latency" "$LOG" | grep -avE "POSIX|v0\.1\.0"
