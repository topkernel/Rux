#!/bin/bash
# abx-s2b.sh — interleaved base/fixed A/B for the review-s2b perf levers
# (lock-free VFS root read, close-side drain/purge trims).
# One case per boot, alternating side per rep so host-load drift hits both.
set -u
cd "$(dirname "$0")"
BASE="./s2b-base-bzImage"
FIXED="./s2b-fixed-bzImage"
IMG="/home/william/rux-agents/perf-path/work/lmbench-x86.img"
OUT="./abx-s2b.txt"
REPS="${1:-5}"
: > "$OUT"
CASES=("lat_syscall open" "lat_syscall stat" "lat_syscall fstat")
for case_spec in "${CASES[@]}"; do
    name="$(echo "$case_spec" | tr ' ,' '__')"
    for rep in $(seq 1 "$REPS"); do
        for side in base fixed; do
            if [ $(( (rep + rep % 2) % 2 )) -eq 1 ]; then :; fi
            waited=0
            while [ "$(cut -d' ' -f1 /proc/loadavg | cut -d. -f1)" -ge 6 ] && [ "$waited" -lt 240 ]; do
                sleep 20; waited=$((waited + 20))
            done
            K="$BASE"; [ "$side" = fixed ] && K="$FIXED"
            log="/tmp/abx-s2b-$$-$name-$side-$rep.log"
            timeout 60 qemu-system-x86_64 -M q35 -accel kvm -cpu host \
                -m 2G -smp 2 -display none -vga none -serial "file:$log" -monitor none \
                -kernel "$K" \
                -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$(echo "$case_spec" | tr ' ' ',') console=ttyS0" \
                -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
                -device virtio-blk-pci,drive=rootfs \
                -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1
            val="$(tr -d '\r' < "$log" 2>/dev/null | grep -aoE '[0-9]+\.[0-9]+ +microseconds' | head -1 | grep -aoE '[0-9]+\.[0-9]+')"
            load="$(cut -d' ' -f1 /proc/loadavg)"
            echo "CASE[$case_spec] $side rep$rep load=$load VAL=${val:-NONE}" >> "$OUT"
            rm -f "$log"
        done
    done
done
echo "abx-s2b done -> $OUT"
