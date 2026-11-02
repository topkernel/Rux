#!/bin/bash
# cg-ab2.sh — interleaved base/fixed boots, LOW PARALLELISM (4 at a time).
# Usage: cg-ab2.sh <bzImage-base> <bzImage-fix> <outfile> <reps> <spec-file>
# Alternates base-first/fix-first per rep; waves of 4 boots.
set -u
WT=/home/william/rux-agents/perf-closegap
BASE="$1"; FIX="$2"; OUT="$3"; REPS="$4"; SPECFILE="$5"
IMG="$WT/work/lmbench-x86.img"
mapfile -t SPECS < "$SPECFILE"
: > "$OUT"

# Build the job list: "kern_tag;spec;rep" per line.
JOBS=/tmp/cg-ab2-jobs.$$
: > "$JOBS"
for r in $(seq 1 "$REPS"); do
    if [ $((r % 2)) -eq 1 ]; then SIDES="base fix"; else SIDES="fix base"; fi
    for side in $SIDES; do
        for spec in "${SPECS[@]}"; do
            [ -n "$spec" ] || continue
            echo "$side;$spec;$r" >> "$JOBS"
        done
    done
done

run_one() { # kern tag spec rep
    local kernel log spec tag rep
    kernel="$1"; tag="$2"; spec="$3"; rep="$4"
    log="/tmp/cg-ab2-${tag}-$(echo "$spec" | tr ' /' '__')-r${rep}.log"
    timeout 55 qemu-system-x86_64 -M q35 -accel kvm -cpu host \
        -m 2G -smp 2 -display none -vga none -serial "file:$log" -monitor none \
        -kernel "$kernel" \
        -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$(echo $spec | tr ' ' ',') budget=40 console=ttyS0" \
        -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
        -device virtio-blk-pci,drive=rootfs >/dev/null 2>&1
}

wave_start=1
total=$(wc -l < "$JOBS")
while [ $wave_start -le $total ]; do
    pids=()
    meta=()
    n=0
    while [ $n -lt 4 ] && [ $((wave_start + n)) -le $total ]; do
        line=$(sed -n "$((wave_start + n))p" "$JOBS")
        tag="${line%%;*}"; rest="${line#*;}"; spec="${rest%%;*}"; rep="${rest##*;}"
        kern="$BASE"; [ "$tag" = fix ] && kern="$FIX"
        run_one "$kern" "$tag" "$spec" "$rep" &
        pids+=($!)
        meta+=("$tag;$spec;$rep")
        n=$((n+1))
    done
    for p in "${pids[@]}"; do wait "$p"; done
    for m in "${meta[@]}"; do
        tag="${m%%;*}"; rest="${m#*;}"; spec="${rest%%;*}"; rep="${rest##*;}"
        log="/tmp/cg-ab2-${tag}-$(echo "$spec" | tr ' /' '__')-r${rep}.log"
        echo "===CASE $spec $tag rep$rep load=$(cut -d' ' -f1 /proc/loadavg)" >> "$OUT"
        tr -d '\r' < "$log" 2>/dev/null \
            | grep -aE "[0-9]+\.[0-9]+|^[0-9]+k |LMBENCH TIMEOUT" | grep -avE "POSIX|v0\.1\.0" >> "$OUT"
        rm -f "$log"
    done
    wave_start=$((wave_start + 4))
done
rm -f "$JOBS"
echo "final load=$(cat /proc/loadavg)" >> "$OUT"
echo done >> "$OUT"
