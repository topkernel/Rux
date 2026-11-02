#!/bin/bash
# cg-cases.sh — per-case boots for the closegap target rows.
# Usage: cg-cases.sh <bzImage> <outfile> [reps]
# Cases: lat_sem, lat_unix, lat_pipe, lat_ctx 2, lat_ctx 8, open
set -u
WT=/home/william/rux-agents/perf-closegap
KERNEL="$1"
OUT="$2"
REPS="${3:-3}"
IMG="$WT/work/lmbench-x86.img"
: > "$OUT"
SPECS=("lat_sem" "lat_unix" "lat_pipe" "lat_ctx -s 64 2" "lat_ctx -s 64 8" "lat_syscall open")
run_one() {
    local spec="$1" tag="$2" log="$3"
    timeout 45 qemu-system-x86_64 -M q35 -accel kvm -cpu host \
        -m 2G -smp 2 -display none -vga none -serial "file:$log" -monitor none \
        -kernel "$KERNEL" \
        -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$(echo $spec | tr ' ' ',') console=ttyS0" \
        -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
        -device virtio-blk-pci,drive=rootfs >/dev/null 2>&1
}
for r in $(seq 1 "$REPS"); do
    pids=()
    for i in "${!SPECS[@]}"; do
        spec="${SPECS[$i]}"
        tag="c${i}r${r}"
        log="/tmp/cg-$tag.log"
        run_one "$spec" "$tag" "$log" &
        pids+=("$!")
    done
    for p in "${pids[@]}"; do wait "$p"; done
    for i in "${!SPECS[@]}"; do
        echo "===CASE ${SPECS[$i]} rep$r" >> "$OUT"
        tr -d '\r' < "/tmp/cg-c${i}r${r}.log" 2>/dev/null \
            | grep -aE "[0-9]+\.[0-9]+|^[0-9]+k |LMBENCH TIMEOUT" | grep -avE "POSIX|v0\.1\.0" >> "$OUT"
        rm -f "/tmp/cg-c${i}r${r}.log"
    done
done
echo "load=$(cat /proc/loadavg)" >> "$OUT"
cat "$OUT"
