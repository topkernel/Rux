#!/bin/bash
# fam-fill.sh — retry the cases that produced no value in a fam-med3 run.
# Usage: fam-fill.sh <tag> [kernel]
set -uo pipefail
WT=/home/william/rux-agents/gap-sched
TAG="${1:?tag}"
KERNEL="${2:-$WT/work/bzImage-lm}"
IMG="$WT/work/lmbench-x86.img"
OUT="$WT/run/fam-$TAG.txt"
CASES=("lat_ctx,-s,64,2" "lat_ctx,-s,64,8" "lat_pipe," "lat_unix," "lat_sem," "lat_syscall,null" "lat_syscall,read" "lat_syscall,write")
for round in 4 5; do
    for i in 0 1 2 3 4 6; do   # skip null (always produced a value)
        spec="${CASES[$i]}"
        log="/tmp/famf-$TAG-$i.log"
        timeout 45 qemu-system-x86_64 -M q35 -accel kvm -cpu host \
            -m 2G -smp 2 -display none -vga none -serial "file:$log" -monitor none \
            -kernel "$KERNEL" \
            -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$spec console=ttyS0" \
            -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
            -device virtio-blk-pci,drive=rootfs >/dev/null 2>&1
        if tr -d '\r' < "$log" 2>/dev/null | grep -aqE "[0-9]+\.[0-9]+"; then
            echo "===CASE r$round ${spec//$/, }" >> "$OUT"
            tr -d '\r' < "$log" | grep -aE "[0-9]+\.[0-9]+" | grep -avE "POSIX|v0\.1\.0" | tail -3 >> "$OUT"
            rm -f "$log"
        else
            # no value; drop the empty log, try next case
            rm -f "$log"
        fi
    done
done
echo "=== fam-fill $TAG done $(date +%H:%M:%S) load=$(cut -d' ' -f1 /proc/loadavg)" >> "$OUT"
