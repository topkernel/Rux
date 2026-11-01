#!/bin/bash
# ab-run.sh — per-case boots of the lmbench rig for A/B perf lanes.
# Usage: ab-run.sh <kernel-bzImage> <label> <reps> <case...>
# Each case boots once per rep (one case per boot: clean serial), 60s cap.
set -u
KERNEL="$1"; LABEL="$2"; REPS="$3"; shift 3
WT="/home/william/rux-agents/perf-path"
IMG="$WT/work/lmbench-x86.img"
OUT="$WT/run/ab-$LABEL.txt"
: > "$OUT"
for case_spec in "$@"; do
    name="$(echo "$case_spec" | tr ' ,' '__')"
    for rep in $(seq 1 "$REPS"); do
        # perf-lane validity rule: a boot measured under loadavg(1m) >= 4
        # is void — wait (bounded) for a quiet window before each boot.
        waited=0
        while [ "$(cut -d' ' -f1 /proc/loadavg | cut -d. -f1)" -ge 4 ] && [ "$waited" -lt 240 ]; do
            sleep 20; waited=$((waited + 20))
        done
        log="/tmp/abp-$$-$name-$rep.log"
        timeout 60 qemu-system-x86_64 -M q35 -accel kvm -cpu host \
            -m 2G -smp 2 -display none -vga none -serial "file:$log" -monitor none \
            -kernel "$KERNEL" \
            -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$(echo "$case_spec" | tr ' ' ',') console=ttyS0" \
            -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
            -device virtio-blk-pci,drive=rootfs \
            -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1
        val="$(tr -d '\r' < "$log" 2>/dev/null | grep -aoE '[0-9]+\.[0-9]+ +microseconds' | head -1 | grep -aoE '[0-9]+\.[0-9]+')"
        load="$(cut -d' ' -f1 /proc/loadavg)"
        echo "CASE[$case_spec] rep$rep load=$load VAL=${val:-NONE}" >> "$OUT"
        rm -f "$log"
    done
done
echo "ab-run done -> $OUT"
