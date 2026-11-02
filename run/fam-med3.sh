#!/bin/bash
# fam-med3.sh — med-of-3 interleaved scheduler-family measurement.
# Cases: the 5 family rows + null/read/write regression guards.
# 3 interleaved rounds (same order each round) so time-of-day host-load
# drift spreads evenly across cases. One boot per case, 45s QEMU cap.
# Usage: fam-med3.sh <tag> [bzImage]
set -uo pipefail
WT=/home/william/rux-agents/gap-sched
TAG="${1:?tag}"
KERNEL="${2:-$WT/work/bzImage-lm}"
IMG="$WT/work/lmbench-x86.img"
OUT="$WT/run/fam-$TAG.txt"

CASES=(
    "lat_ctx,-s,64,2"   # 0
    "lat_ctx,-s,64,8"   # 1
    "lat_pipe,"         # 2
    "lat_unix,"         # 3
    "lat_sem,"          # 4
    "lat_syscall,null"  # 5 guard
    "lat_syscall,read"  # 6 guard
    "lat_syscall,write" # 7 guard
)
NC=${#CASES[@]}

echo "=== fam-med3 $TAG $(date +%F_%H:%M:%S) kernel=$(git -C "$WT" log --oneline -1 | cut -c1-8)" > "$OUT"

run_case() { # idx -> log
    local spec="${CASES[$1]}"
    local log="/tmp/fam3-$TAG-$1.log"
    timeout 45 qemu-system-x86_64 -M q35 -accel kvm -cpu host \
        -m 2G -smp 2 -display none -vga none -serial "file:$log" -monitor none \
        -kernel "$KERNEL" \
        -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$spec console=ttyS0" \
        -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
        -device virtio-blk-pci,drive=rootfs >/dev/null 2>&1
}

for round in 1 2 3; do
    echo "--- round $round load=$(cut -d' ' -f1 /proc/loadavg)" >> "$OUT"
    for i in $(seq 0 $((NC-1))); do
        run_case "$i"
        echo "===CASE r$round ${CASES[$i]//$/, }" >> "$OUT"
        tr -d '\r' < "/tmp/fam3-$TAG-$i.log" 2>/dev/null \
            | grep -aE "[0-9]+\.[0-9]+|LMBENCH (TIMEOUT|EXEC-FAIL)" \
            | grep -avE "POSIX|v0\.1\.0" | tail -4 >> "$OUT"
        rm -f "/tmp/fam3-$TAG-$i.log"
    done
done
echo "=== fam-med3 $TAG done $(date +%H:%M:%S)" >> "$OUT"
cat "$OUT"
