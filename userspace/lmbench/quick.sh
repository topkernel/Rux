#!/bin/bash
# quick.sh — the 3-minute benchmark answer (user rule, 2026-11-02).
#
# Runs the 10 headline lmbench cases on the CURRENT kernel, 2 boots in
# parallel, 25s per case, NO host-load gate (prints the load and marks
# the numbers INDICATIVE). Deltas > 10% are real; smaller is noise.
# For gate-grade standings use run-rux.sh med-of-runs in a quiet window.
#
# Total wall time: 1.5-2.5 min (load <5: all rows; load >7: some may drop and are retried once) (build cached, image reflink-copied).
# Usage: quick.sh [bzImage]        (default: strong release build)
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WT="$(cd "$HERE/../.." && pwd)"
KERNEL="${1:-$WT/work/bzImage-lm}"
IMG="$WT/work/lmbench-x86.img"
OUT="${2:-$WT/run/lmbench-quick.txt}"

# One-time setup (all cached after the first run):
[ -f "$KERNEL" ] || {
    make -C "$WT/build" build-release-opt PLATFORM=x86_64 >/dev/null 2>&1
    cp "$WT/target/x86_64-unknown-none/release/bzImage" "$KERNEL"
}
[ -f "$IMG" ] || cp --reflink=auto /home/william/rux-images/testimages/lmbench-x86.img "$IMG"

# Headline cases (the rows that move when the kernel changes).
CASES=(
    "lat_syscall,null" "lat_syscall,read" "lat_syscall,write"
    "lat_syscall,stat" "lat_syscall,fstat" "lat_syscall,open"
    "lat_proc,fork" "lat_proc,exec" "lat_pipe," "lat_ctx,-s,64,2"
)
N="${#CASES[@]}"

BUDGET=30
run_case() {
    local spec="$1" idx="$2"
    local log="/tmp/lmq-$idx.log"
    timeout $BUDGET qemu-system-x86_64 -M q35 -accel kvm -cpu host \
        -m 2G -smp 2 -display none -vga none -serial "file:$log" -monitor none \
        -kernel "$KERNEL" \
        -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$spec console=ttyS0" \
        -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
        -device virtio-blk-pci,drive=rootfs >/dev/null 2>&1
}
has_result() {  # a case produced a measurement line (banner v0.1.0 does NOT count)
    tr -d '\r' < "/tmp/lmq-$1.log" 2>/dev/null \
        | grep -avE "POSIX|v0\.1\.0" | grep -aqE "[0-9]+\.[0-9]+"
}

echo "=== lmbench QUICK $(date +%H:%M:%S) load=$(cut -d' ' -f1 /proc/loadavg) kernel=$(git -C "$WT" log --oneline -1 | cut -c1-8) [INDICATIVE — no load gate]" > "$OUT"
START=$SECONDS
# Wave 1: three boots in parallel (~4 waves x 30s = 120s worst).
i=0
while [ $i -lt $N ]; do
    pids=()
    for j in 0 1 2; do
        [ $((i+j)) -lt $N ] || break
        run_case "${CASES[$((i+j))]}" "$((i+j))" &
        pids+=($!)
    done
    for p in "${pids[@]}"; do wait "$p"; done
    i=$((i+3))
done
# Wave 2: one serial retry for rows the load starved.
for k in $(seq 0 $((N-1))); do
    has_result $k || run_case "${CASES[$k]}" "$k"
done
for k in $(seq 0 $((N-1))); do
    log="/tmp/lmq-$k.log"
    echo "===CASE ${CASES[$k]//$/, }" >> "$OUT"
    tr -d '\r' < "$log" 2>/dev/null | grep -aE "[0-9]+\.[0-9]+|LMBENCH TIMEOUT" | grep -avE "POSIX|v0\.1\.0" >> "$OUT"
    rm -f "$log"
done
echo "=== wall ${SECONDS}[-$START]s (should be < 180)" >> "$OUT"
cat "$OUT"
