#!/bin/bash
# gn-ab.sh — interleaved base/fix boots for the gap-net target rows.
# Usage: gn-ab.sh <bzImage-base> <bzImage-fix> <outfile> [reps]
# Cases: bw_unix, lat_unix, lat_syscall null/read/write, lat_pipe (guard).
set -u
WT=/home/william/rux-agents/gap-net
BASE="$1"; FIX="$2"; OUT="$3"; REPS="${4:-4}"
IMG="$WT/work/lmbench-x86.img"
: > "$OUT"
SPECS=("bw_unix" "lat_unix" "lat_syscall,null" "lat_syscall,read" "lat_syscall,write" "lat_pipe")
run_one() {
    local kernel="$1" spec="$2" log="$3"
    timeout 50 qemu-system-x86_64 -M q35 -accel kvm -cpu host \
        -m 2G -smp 2 -display none -vga none -serial "file:$log" -monitor none \
        -kernel "$kernel" \
        -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$spec budget=40 console=ttyS0" \
        -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
        -device virtio-blk-pci,drive=rootfs >/dev/null 2>&1
}
for r in $(seq 1 "$REPS"); do
    if [ $((r % 2)) -eq 1 ]; then ORDER=("$BASE" "$FIX"); TAGS=(base fix)
    else ORDER=("$FIX" "$BASE"); TAGS=(fix base); fi
    for k in 0 1; do
        kern="${ORDER[$k]}"; tag="${TAGS[$k]}"
        pids=()
        for i in "${!SPECS[@]}"; do
            log="/tmp/gnab-${tag}-c${i}r${r}.log"
            run_one "$kern" "${SPECS[$i]}" "$log" &
            pids+=("$!")
        done
        for p in "${pids[@]}"; do wait "$p"; done
        for i in "${!SPECS[@]}"; do
            echo "===CASE ${SPECS[$i]} $tag rep$r load=$(cut -d' ' -f1 /proc/loadavg)" >> "$OUT"
            tr -d '\r' < "/tmp/gnab-${tag}-c${i}r${r}.log" 2>/dev/null \
                | grep -aE "[0-9]+\.[0-9]+|^[0-9]+k |LMBENCH TIMEOUT" | grep -avE "POSIX|v0\.1\.0" >> "$OUT"
            rm -f "/tmp/gnab-${tag}-c${i}r${r}.log"
        done
    done
done
echo "final load=$(cat /proc/loadavg)" >> "$OUT"
echo done
