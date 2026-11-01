#!/bin/bash
# sysab.sh — syscall-family A/B rig (queue item 1: syscall entry/exit).
# One boot per case (same shape as userspace/lmbench/run-rux.sh), the
# syscall family only, N sweeps, med-of-sweeps verdict. Records host
# load + foreign qemu count per sweep; a sweep at load>=4 is VOID.
#
# Usage: run/sysab.sh <tag> [nsweeps]
#   -> run/sysab-<tag>.txt
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WT="$(cd "$HERE/.." && pwd)"
KERNEL="${KERNEL:-$WT/target/x86_64-unknown-none/release/bzImage}"
IMG="$WT/work/lmbench-x86.img"
TAG="${1:?usage: sysab.sh <tag> [nsweeps]}"
N="${2:-3}"
CASES=("lat_syscall null" "lat_syscall read" "lat_syscall write" "lat_syscall fstat" "lat_syscall open")
OUT="$WT/run/sysab-$TAG.txt"
: > "$OUT"
for sweep in $(seq 1 "$N"); do
    load1=$(cut -d' ' -f1 /proc/loadavg)
    fq=$(ps aux | grep -c "[q]emu")
    echo "# sweep $sweep load1=$load1 foreign_qemus=$fq kernel=$(md5sum "$KERNEL" | cut -c1-8) $(date +%H:%M:%S)" >> "$OUT"
    for spec in "${CASES[@]}"; do
        name=$(echo "$spec" | tr ' ' '_')
        val=""
        for attempt in 1 2; do
            log="/tmp/sysab-$name-$$.log"
            timeout 90 qemu-system-x86_64 -M q35 -accel kvm -cpu host \
                -m 2G -smp 2 -display none -vga none -serial "file:$log" -monitor none \
                -kernel "$KERNEL" \
                -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$(echo "$spec" | tr ' ' ',') console=ttyS0" \
                -drive file="$IMG",if=none,id=rootfs,format=raw \
                -device virtio-blk-pci,drive=rootfs \
                -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1
            val=$(tr -d '\r' < "$log" 2>/dev/null | grep -aoE "Simple [a-z ]+: [0-9]+\.[0-9]+ microseconds" | tail -1)
            rm -f "$log"
            [ -n "$val" ] && break
        done
        echo "$name | $val" >> "$OUT"
    done
done
echo "sysab: -> $OUT"
